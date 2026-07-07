use crate::client::CondoClient;
use crate::vfs::{Attr, Vfs};
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry, Request,
};
use std::ffi::OsStr;
use std::time::Duration;

const TTL: Duration = Duration::from_secs(1);

pub struct CondoFs<C: CondoClient> {
    vfs: Vfs<C>,
}

impl<C: CondoClient> CondoFs<C> {
    pub fn new(vfs: Vfs<C>) -> CondoFs<C> {
        CondoFs { vfs }
    }
}

fn to_file_attr(a: &Attr, uid: u32, gid: u32) -> FileAttr {
    let (perm, nlink) = match a.kind {
        FileType::Directory => (0o555, 2),
        _ => (0o444, 1),
    };
    let blocks = a.size.div_ceil(512);
    FileAttr {
        ino: a.ino,
        size: a.size,
        blocks,
        atime: a.mtime,
        mtime: a.mtime,
        ctime: a.mtime,
        crtime: a.mtime,
        kind: a.kind,
        perm,
        nlink,
        uid,
        gid,
        rdev: 0,
        blksize: 512,
        flags: 0,
    }
}

impl<C: CondoClient> Filesystem for CondoFs<C> {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name = match name.to_str() {
            Some(n) => n,
            None => return reply.error(libc::ENOENT),
        };
        match self.vfs.lookup(parent, name) {
            Ok(attr) => reply.entry(
                &TTL,
                &to_file_attr(&attr, self.vfs.uid(), self.vfs.gid()),
                0,
            ),
            Err(e) => reply.error(e.errno()),
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        match self.vfs.getattr(ino) {
            Ok(attr) => reply.attr(&TTL, &to_file_attr(&attr, self.vfs.uid(), self.vfs.gid())),
            Err(e) => reply.error(e.errno()),
        }
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        let offset = if offset < 0 { 0 } else { offset as u64 };
        match self.vfs.read(ino, offset, size) {
            Ok(data) => reply.data(&data),
            Err(e) => reply.error(e.errno()),
        }
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let entries = match self.vfs.readdir(ino) {
            Ok(e) => e,
            Err(e) => return reply.error(e.errno()),
        };
        // "." and ".." first, then the real children.
        let mut listing: Vec<(u64, FileType, String)> = vec![
            (ino, FileType::Directory, ".".to_string()),
            (ino, FileType::Directory, "..".to_string()),
        ];
        for e in entries {
            listing.push((e.ino, e.kind, e.name));
        }
        for (i, (child_ino, kind, name)) in listing.into_iter().enumerate().skip(offset as usize) {
            // `add` returns true when the reply buffer is full.
            if reply.add(child_ino, (i + 1) as i64, kind, &name) {
                break;
            }
        }
        reply.ok();
    }
}
