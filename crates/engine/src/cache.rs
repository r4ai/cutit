use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use crate::preview::PreviewFrame;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PreviewCacheKey {
    path: PathBuf,
    bucket: i64,
}

/// LRU cache for decoded preview frames bucketed by source timeline ticks.
///
/// # Example
/// ```
/// use std::sync::Arc;
///
/// use engine::cache::PreviewFrameCache;
/// use engine::{PreviewFrame, PreviewPixelFormat};
///
/// let mut cache = PreviewFrameCache::new(8, 33_333);
/// cache.insert(
///     "demo.mp4",
///     1_500_000,
///     PreviewFrame {
///         width: 2,
///         height: 2,
///         format: PreviewPixelFormat::Rgba8,
///         bytes: Arc::from(vec![0; 16]),
///     },
/// );
///
/// assert!(cache.get("demo.mp4", 1_500_010).is_some());
/// ```
#[derive(Debug)]
pub struct PreviewFrameCache {
    capacity: usize,
    bucket_size_tl: i64,
    entries: HashMap<PreviewCacheKey, PreviewFrame>,
    lru_order: VecDeque<PreviewCacheKey>,
}

impl PreviewFrameCache {
    /// Creates a preview cache.
    ///
    /// `capacity` and `bucket_size_tl` must be positive.
    pub fn new(capacity: usize, bucket_size_tl: i64) -> Self {
        assert!(capacity > 0, "preview cache capacity must be positive");
        assert!(
            bucket_size_tl > 0,
            "preview cache bucket size must be positive"
        );
        Self {
            capacity,
            bucket_size_tl,
            entries: HashMap::new(),
            lru_order: VecDeque::new(),
        }
    }

    /// Clears all cached frames.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.lru_order.clear();
    }

    /// Returns cache bucket size in timeline ticks.
    pub fn bucket_size_tl(&self) -> i64 {
        self.bucket_size_tl
    }

    /// Returns true when a frame for the same key bucket already exists.
    pub fn contains(&self, path: impl AsRef<Path>, source_tl: i64) -> bool {
        let key = self.make_key(path.as_ref(), source_tl);
        self.entries.contains_key(&key)
    }

    /// Returns one cached frame and marks it as recently used.
    pub fn get(&mut self, path: impl AsRef<Path>, source_tl: i64) -> Option<PreviewFrame> {
        let key = self.make_key(path.as_ref(), source_tl);
        let frame = self.entries.get(&key)?.clone();
        self.touch(&key);
        Some(frame)
    }

    /// Inserts or updates one cached frame.
    pub fn insert(&mut self, path: impl AsRef<Path>, source_tl: i64, frame: PreviewFrame) {
        let key = self.make_key(path.as_ref(), source_tl);
        self.entries.insert(key.clone(), frame);
        self.touch(&key);
        self.evict_if_needed();
    }

    fn make_key(&self, path: &Path, source_tl: i64) -> PreviewCacheKey {
        PreviewCacheKey {
            path: path.to_path_buf(),
            bucket: source_tl.max(0).div_euclid(self.bucket_size_tl),
        }
    }

    fn touch(&mut self, key: &PreviewCacheKey) {
        if let Some(index) = self.lru_order.iter().position(|existing| existing == key) {
            let _ = self.lru_order.remove(index);
        }
        self.lru_order.push_back(key.clone());
    }

    fn evict_if_needed(&mut self) {
        while self.entries.len() > self.capacity {
            let Some(oldest) = self.lru_order.pop_front() else {
                break;
            };
            let _ = self.entries.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::preview::PreviewPixelFormat;

    use super::PreviewFrameCache;

    #[test]
    fn get_hits_for_timestamps_in_the_same_bucket() {
        let mut cache = PreviewFrameCache::new(8, 33_333);
        cache.insert("demo.mp4", 1_500_000, sample_frame(10));

        let frame = cache
            .get("demo.mp4", 1_500_010)
            .expect("frame should be cached");
        assert_eq!(frame.bytes[0], 10);
    }

    #[test]
    fn insert_evicts_least_recently_used_frame_when_capacity_is_reached() {
        let mut cache = PreviewFrameCache::new(2, 33_333);
        cache.insert("demo.mp4", 1_000_000, sample_frame(1));
        cache.insert("demo.mp4", 2_000_000, sample_frame(2));

        let _ = cache
            .get("demo.mp4", 1_000_000)
            .expect("first frame should exist");
        cache.insert("demo.mp4", 3_000_000, sample_frame(3));

        assert!(cache.get("demo.mp4", 1_000_000).is_some());
        assert!(cache.get("demo.mp4", 2_000_000).is_none());
        assert!(cache.get("demo.mp4", 3_000_000).is_some());
    }

    fn sample_frame(value: u8) -> crate::preview::PreviewFrame {
        crate::preview::PreviewFrame {
            width: 1,
            height: 1,
            format: PreviewPixelFormat::Rgba8,
            bytes: Arc::from(vec![value; 4]),
        }
    }
}
