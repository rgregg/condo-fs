use crate::cache::ContentCache;
use crate::client::{ClientError, CondoClient};
use crate::model::Entry;
use crate::names;
use fuser::FileType;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsError {
    NotFound,
    Io,
}

impl VfsError {
    pub fn errno(&self) -> i32 {
        match self {
            VfsError::NotFound => libc::ENOENT,
            VfsError::Io => libc::EIO,
        }
    }
}

impl From<ClientError> for VfsError {
    fn from(e: ClientError) -> Self {
        match e {
            ClientError::NotFound => VfsError::NotFound,
            _ => VfsError::Io,
        }
    }
}
impl From<std::io::Error> for VfsError {
    fn from(_: std::io::Error) -> Self {
        VfsError::Io
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub ino: u64,
    pub name: String,
    pub kind: FileType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attr {
    pub ino: u64,
    pub size: u64,
    pub kind: FileType,
    pub mtime: SystemTime,
}

#[derive(Clone)]
enum Node {
    Folder {
        id: u64,
    },
    File {
        id: u64,
        date: String,
        #[allow(dead_code)]
        thumbnail: String,
        size: Option<u64>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum NodeKey {
    Folder(u64),
    File(u64),
}

struct Inner {
    next_ino: u64,
    nodes: HashMap<u64, Node>,
    id_to_ino: HashMap<NodeKey, u64>,
    child_by_name: HashMap<(u64, String), u64>,
    listed_at: HashMap<u64, Instant>,
}

pub struct Vfs<C: CondoClient> {
    client: C,
    cache: ContentCache,
    meta_ttl: Duration,
    uid: u32,
    gid: u32,
    inner: Mutex<Inner>,
}

impl<C: CondoClient> Vfs<C> {
    pub fn new(
        client: C,
        root_folder_id: u64,
        cache: ContentCache,
        meta_ttl: Duration,
        uid: u32,
        gid: u32,
    ) -> Vfs<C> {
        let mut nodes = HashMap::new();
        nodes.insert(fuser::FUSE_ROOT_ID, Node::Folder { id: root_folder_id });
        let mut id_to_ino = HashMap::new();
        id_to_ino.insert(NodeKey::Folder(root_folder_id), fuser::FUSE_ROOT_ID);
        Vfs {
            client,
            cache,
            meta_ttl,
            uid,
            gid,
            inner: Mutex::new(Inner {
                next_ino: 2,
                nodes,
                id_to_ino,
                child_by_name: HashMap::new(),
                listed_at: HashMap::new(),
            }),
        }
    }

    pub fn uid(&self) -> u32 {
        self.uid
    }
    pub fn gid(&self) -> u32 {
        self.gid
    }

    fn attr_for(&self, ino: u64, node: &Node) -> Attr {
        match node {
            Node::Folder { .. } => Attr {
                ino,
                size: 0,
                kind: FileType::Directory,
                mtime: SystemTime::UNIX_EPOCH,
            },
            Node::File { date, size, .. } => Attr {
                ino,
                size: size.unwrap_or(0),
                kind: FileType::RegularFile,
                mtime: names::parse_condo_date(date),
            },
        }
    }

    fn ensure_listed(&self, ino: u64) -> Result<(), VfsError> {
        let folder_id = {
            let inner = self.inner.lock().unwrap();
            match inner.nodes.get(&ino) {
                Some(Node::Folder { id }) => *id,
                Some(Node::File { .. }) => return Err(VfsError::Io), // not a dir
                None => return Err(VfsError::NotFound),
            }
        };
        {
            let inner = self.inner.lock().unwrap();
            if let Some(at) = inner.listed_at.get(&ino) {
                if at.elapsed() <= self.meta_ttl {
                    return Ok(());
                }
            }
        }
        // Network call with no lock held.
        let entries = self.client.list_folder(folder_id)?;

        // Build display names in listing order, then resolve collisions.
        let display: Vec<String> = entries
            .iter()
            .map(|e| match e {
                Entry::Folder { name, .. } => names::sanitize_name(name),
                Entry::File {
                    name, thumbnail, ..
                } => names::file_display_name(name, thumbnail),
            })
            .collect();
        let display = names::resolve_collisions(display);

        let mut inner = self.inner.lock().unwrap();
        // Clear the stale name index for this dir before rebuilding.
        inner.child_by_name.retain(|(p, _), _| *p != ino);
        for (entry, name) in entries.into_iter().zip(display) {
            let (key, node) = match entry {
                Entry::Folder { id, .. } => (NodeKey::Folder(id), Node::Folder { id }),
                Entry::File {
                    id,
                    date,
                    thumbnail,
                    ..
                } => {
                    // Preserve a previously-learned size if we already know this file.
                    let prev_size = inner
                        .id_to_ino
                        .get(&NodeKey::File(id))
                        .and_then(|ci| inner.nodes.get(ci))
                        .and_then(|n| {
                            if let Node::File { size, .. } = n {
                                *size
                            } else {
                                None
                            }
                        });
                    (
                        NodeKey::File(id),
                        Node::File {
                            id,
                            date,
                            thumbnail,
                            size: prev_size,
                        },
                    )
                }
            };
            let child_ino = match inner.id_to_ino.get(&key) {
                Some(ci) => *ci,
                None => {
                    let ci = inner.next_ino;
                    inner.next_ino += 1;
                    inner.id_to_ino.insert(key, ci);
                    ci
                }
            };
            inner.nodes.insert(child_ino, node);
            inner.child_by_name.insert((ino, name), child_ino);
        }
        inner.listed_at.insert(ino, Instant::now());
        Ok(())
    }

    pub fn readdir(&self, ino: u64) -> Result<Vec<DirEntry>, VfsError> {
        self.ensure_listed(ino)?;
        let inner = self.inner.lock().unwrap();
        let mut pairs: Vec<(String, u64)> = inner
            .child_by_name
            .iter()
            .filter(|((p, _), _)| *p == ino)
            .map(|((_, n), ci)| (n.clone(), *ci))
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out = Vec::with_capacity(pairs.len());
        for (name, ci) in pairs {
            let kind = match inner.nodes.get(&ci) {
                Some(Node::Folder { .. }) => FileType::Directory,
                _ => FileType::RegularFile,
            };
            out.push(DirEntry {
                ino: ci,
                name,
                kind,
            });
        }
        Ok(out)
    }

    pub fn lookup(&self, parent: u64, name: &str) -> Result<Attr, VfsError> {
        self.ensure_listed(parent)?;
        let (ino, node) = {
            let inner = self.inner.lock().unwrap();
            let ino = *inner
                .child_by_name
                .get(&(parent, name.to_string()))
                .ok_or(VfsError::NotFound)?;
            let node = inner.nodes.get(&ino).ok_or(VfsError::NotFound)?.clone();
            (ino, node)
        };
        self.ensure_size(ino, &node)
    }

    pub fn getattr(&self, ino: u64) -> Result<Attr, VfsError> {
        let node = {
            let inner = self.inner.lock().unwrap();
            inner.nodes.get(&ino).ok_or(VfsError::NotFound)?.clone()
        };
        self.ensure_size(ino, &node)
    }

    /// For a file whose size we do not yet know, fetch it (getattr/lookup need a real size).
    fn ensure_size(&self, ino: u64, node: &Node) -> Result<Attr, VfsError> {
        if let Node::File { id, size: None, .. } = node {
            let meta = self.client.file_meta(*id)?;
            let mut inner = self.inner.lock().unwrap();
            if let Some(Node::File { size, .. }) = inner.nodes.get_mut(&ino) {
                *size = Some(meta.size);
            }
            let updated = inner.nodes.get(&ino).cloned().ok_or(VfsError::NotFound)?;
            return Ok(self.attr_for(ino, &updated));
        }
        Ok(self.attr_for(ino, node))
    }

    pub fn read(&self, ino: u64, offset: u64, size: u32) -> Result<Vec<u8>, VfsError> {
        let (id, date) = {
            let inner = self.inner.lock().unwrap();
            match inner.nodes.get(&ino) {
                Some(Node::File { id, date, .. }) => (*id, date.clone()),
                Some(Node::Folder { .. }) => return Err(VfsError::Io),
                None => return Err(VfsError::NotFound),
            }
        };
        // Ensure the file is cached on disk (download the whole file once; the
        // server ignores Range requests, so partial reads are not possible).
        let path = match self.cache.get(id, &date) {
            Some(p) => p,
            None => self.cache.store_from(id, &date, |f| {
                self.client
                    .download_file(id, f)
                    .map_err(|_| std::io::Error::other("download failed"))?;
                Ok(())
            })?,
        };
        let mut file = std::fs::File::open(&path)?;
        let len = file.metadata()?.len();
        if offset >= len {
            return Ok(Vec::new());
        }
        file.seek(SeekFrom::Start(offset))?;
        let want = std::cmp::min(size as u64, len - offset) as usize;
        let mut buf = vec![0u8; want];
        let n = read_full(&mut file, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }
}

fn read_full<R: Read>(r: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FileMeta;
    use std::io::Write;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct MockClient {
        folders: HashMap<u64, Vec<Entry>>,
        files: HashMap<u64, (Vec<u8>, u64)>, // id -> (bytes, size)
        meta_calls: AtomicU32,
        download_calls: AtomicU32,
    }

    impl CondoClient for MockClient {
        fn login(&self) -> Result<(), ClientError> {
            Ok(())
        }
        fn list_folder(&self, folder_id: u64) -> Result<Vec<Entry>, ClientError> {
            Ok(self.folders.get(&folder_id).cloned().unwrap_or_default())
        }
        fn file_meta(&self, file_id: u64) -> Result<FileMeta, ClientError> {
            self.meta_calls.fetch_add(1, Ordering::SeqCst);
            let (_, size) = self.files.get(&file_id).ok_or(ClientError::NotFound)?;
            Ok(FileMeta {
                size: *size,
                filename: None,
            })
        }
        fn download_file(&self, file_id: u64, out: &mut dyn Write) -> Result<u64, ClientError> {
            self.download_calls.fetch_add(1, Ordering::SeqCst);
            let (bytes, _) = self.files.get(&file_id).ok_or(ClientError::NotFound)?;
            out.write_all(bytes)?;
            Ok(bytes.len() as u64)
        }
    }

    fn fixture_vfs() -> (Vfs<MockClient>, tempfile::TempDir) {
        // root folder 100 contains subfolder 200 ("Reports") and file 300 ("01/09/25 Notes", pdf)
        let mut folders = HashMap::new();
        folders.insert(
            100,
            vec![
                Entry::Folder {
                    id: 200,
                    name: "Reports".into(),
                },
                Entry::File {
                    id: 300,
                    key: "K".into(),
                    name: "01/09/25 Notes".into(),
                    date: "2025-01-18 02:41:25".into(),
                    thumbnail: "/shared/images/icons/pdf-128x128.png".into(),
                },
            ],
        );
        folders.insert(200, vec![]);
        let mut files = HashMap::new();
        files.insert(300u64, (b"%PDF-1.7 body".to_vec(), 13u64));
        let client = MockClient {
            folders,
            files,
            meta_calls: AtomicU32::new(0),
            download_calls: AtomicU32::new(0),
        };
        let dir = tempfile::tempdir().unwrap();
        let cache = ContentCache::new(dir.path().to_path_buf()).unwrap();
        (
            Vfs::new(client, 100, cache, Duration::from_secs(60), 1000, 1000),
            dir,
        )
    }

    #[test]
    fn readdir_root_lists_folder_and_file_with_clean_names() {
        let (vfs, _d) = fixture_vfs();
        let mut entries = vfs.readdir(fuser::FUSE_ROOT_ID).unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(
            names,
            vec!["01-09-25 Notes.pdf".to_string(), "Reports".to_string()]
        );
        let file = entries.iter().find(|e| e.name.ends_with(".pdf")).unwrap();
        assert_eq!(file.kind, FileType::RegularFile);
    }

    #[test]
    fn lookup_then_read_returns_file_bytes() {
        let (vfs, _d) = fixture_vfs();
        let attr = vfs
            .lookup(fuser::FUSE_ROOT_ID, "01-09-25 Notes.pdf")
            .unwrap();
        assert_eq!(attr.kind, FileType::RegularFile);
        assert_eq!(attr.size, 13);
        let data = vfs.read(attr.ino, 0, 100).unwrap();
        assert_eq!(data, b"%PDF-1.7 body");
        // offset read
        let tail = vfs.read(attr.ino, 9, 100).unwrap();
        assert_eq!(tail, b"body");
    }

    #[test]
    fn lookup_missing_is_enoent() {
        let (vfs, _d) = fixture_vfs();
        let err = vfs.lookup(fuser::FUSE_ROOT_ID, "nope").unwrap_err();
        assert_eq!(err.errno(), libc::ENOENT);
    }

    #[test]
    fn inodes_are_stable_across_relisting() {
        let (vfs, _d) = fixture_vfs();
        let a = vfs.lookup(fuser::FUSE_ROOT_ID, "Reports").unwrap().ino;
        let _ = vfs.readdir(fuser::FUSE_ROOT_ID).unwrap();
        let b = vfs.lookup(fuser::FUSE_ROOT_ID, "Reports").unwrap().ino;
        assert_eq!(a, b);
    }
}
