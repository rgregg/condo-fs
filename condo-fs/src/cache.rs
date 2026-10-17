use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct ContentCache {
    root: PathBuf,
}

impl ContentCache {
    pub fn new(root: PathBuf) -> std::io::Result<ContentCache> {
        std::fs::create_dir_all(&root)?;
        Ok(ContentCache { root })
    }

    fn key(file_id: u64, date: &str) -> String {
        // date may contain spaces/colons; make it filesystem-safe.
        let safe: String = date
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        format!("{file_id}-{safe}.bin")
    }

    pub fn path_for(&self, file_id: u64, date: &str) -> PathBuf {
        self.root.join(Self::key(file_id, date))
    }

    pub fn get(&self, file_id: u64, date: &str) -> Option<PathBuf> {
        let p = self.path_for(file_id, date);
        if p.exists() {
            Some(p)
        } else {
            None
        }
    }

    /// Write via a temp file then atomically rename, so a partial download never
    /// looks like a complete cache entry.
    pub fn store_from<F>(&self, file_id: u64, date: &str, fill: F) -> std::io::Result<PathBuf>
    where
        F: FnOnce(&mut std::fs::File) -> std::io::Result<()>,
    {
        let final_path = self.path_for(file_id, date);
        let tmp = self
            .root
            .join(format!(".tmp-{file_id}-{}", std::process::id()));
        {
            let mut f = std::fs::File::create(&tmp)?;
            fill(&mut f)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &final_path)?;
        Ok(final_path)
    }
}

pub struct TtlCache<K, V> {
    ttl: Duration,
    map: Mutex<HashMap<K, (Instant, V)>>,
}

impl<K: std::hash::Hash + Eq + Clone, V: Clone> TtlCache<K, V> {
    pub fn new(ttl: Duration) -> TtlCache<K, V> {
        TtlCache {
            ttl,
            map: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, k: &K) -> Option<V> {
        let mut map = self.map.lock().unwrap();
        if let Some((at, v)) = map.get(k) {
            if at.elapsed() <= self.ttl {
                return Some(v.clone());
            }
            map.remove(k);
        }
        None
    }

    pub fn put(&self, k: K, v: V) {
        self.map.lock().unwrap().insert(k, (Instant::now(), v));
    }

    pub fn invalidate(&self, k: &K) {
        self.map.lock().unwrap().remove(k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_cache_stores_and_hits_by_id_and_date() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ContentCache::new(dir.path().to_path_buf()).unwrap();
        assert!(cache.get(1, "2025-01-01 00:00:00").is_none());
        let p = cache
            .store_from(1, "2025-01-01 00:00:00", |f| {
                use std::io::Write;
                f.write_all(b"data")
            })
            .unwrap();
        assert!(p.exists());
        assert_eq!(std::fs::read(&p).unwrap(), b"data");
        // same id+date -> hit
        assert!(cache.get(1, "2025-01-01 00:00:00").is_some());
        // changed date -> miss (file was re-uploaded)
        assert!(cache.get(1, "2025-02-02 00:00:00").is_none());
    }

    #[test]
    fn ttl_cache_expires() {
        let c: TtlCache<u64, String> = TtlCache::new(Duration::from_millis(30));
        c.put(7, "hi".into());
        assert_eq!(c.get(&7), Some("hi".to_string()));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(c.get(&7), None);
    }

    #[test]
    fn ttl_cache_invalidate() {
        let c: TtlCache<u64, String> = TtlCache::new(Duration::from_secs(60));
        c.put(1, "a".into());
        c.invalidate(&1);
        assert_eq!(c.get(&1), None);
    }
}
