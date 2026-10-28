//! The decoded-bitmap cache with a hard memory cap.
//!
//! The GPU keeps display copies of items, and assets keep their encoded
//! originals and previews, but a full-resolution bitmap is only needed while an
//! image is being (re)uploaded or examined. Those bitmaps are the memory hogs —
//! an 8000 x 8000 PNG is 256 MB decoded — so they live in an LRU cache capped at
//! two gigabytes by default.
//!
//! Evicting is safe precisely because every asset carries a preview: an item
//! whose full bitmap was evicted still draws, and the next time it becomes
//! visible the importer can decode it again.

use std::collections::HashMap;
use std::sync::Arc;

use rizref_codecs::ImageBuffer;
use rizref_core::AssetId;

/// Default cap on decoded full-resolution pixels, in bytes.
pub const DEFAULT_DECODE_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// One cached bitmap with its LRU bookkeeping.
#[derive(Debug, Clone)]
struct Entry {
    image: Arc<ImageBuffer>,
    bytes: u64,
    last_used: u64,
}

/// An LRU cache of full-resolution decoded bitmaps keyed by asset.
#[derive(Debug)]
pub struct DecodeCache {
    entries: HashMap<AssetId, Entry>,
    bytes: u64,
    max_bytes: u64,
    clock: u64,
}

impl DecodeCache {
    /// A cache holding at most `max_bytes` of decoded pixels.
    pub fn new(max_bytes: u64) -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            max_bytes,
            clock: 0,
        }
    }

    /// The cache at the default two-gigabyte cap.
    pub fn with_default_cap() -> Self {
        Self::new(DEFAULT_DECODE_CACHE_BYTES)
    }

    /// Stores `image`, evicting least-recently-used entries to respect the cap.
    ///
    /// Returns the evicted assets so the caller can log or re-plan uploads.
    /// A single image larger than the whole cap is kept: refusing it would make
    /// its item permanently unrenderable, which is worse than briefly exceeding
    /// a soft budget.
    pub fn insert(&mut self, asset: AssetId, image: Arc<ImageBuffer>) -> Vec<AssetId> {
        let bytes = byte_size(&image);
        self.clock += 1;
        if let Some(previous) = self.entries.insert(
            asset,
            Entry {
                image,
                bytes,
                last_used: self.clock,
            },
        ) {
            self.bytes = self.bytes.saturating_sub(previous.bytes);
        }
        self.bytes = self.bytes.saturating_add(bytes);

        let mut evicted = Vec::new();
        while self.bytes > self.max_bytes && self.entries.len() > 1 {
            let Some(victim) = self
                .entries
                .iter()
                .filter(|(id, _)| **id != asset)
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(id, _)| *id)
            else {
                break;
            };
            if let Some(entry) = self.entries.remove(&victim) {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
                evicted.push(victim);
            }
        }
        evicted
    }

    /// The bitmap for `asset`, refreshing its recency.
    pub fn get(&mut self, asset: AssetId) -> Option<Arc<ImageBuffer>> {
        self.clock += 1;
        let entry = self.entries.get_mut(&asset)?;
        entry.last_used = self.clock;
        Some(Arc::clone(&entry.image))
    }

    /// Whether `asset` has a full-resolution bitmap cached.
    #[cfg(test)]
    pub fn contains(&self, asset: AssetId) -> bool {
        self.entries.contains_key(&asset)
    }

    /// Drops one asset's bitmap, returning whether it was present.
    #[cfg(test)]
    pub fn remove(&mut self, asset: AssetId) -> bool {
        match self.entries.remove(&asset) {
            Some(entry) => {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
                true
            }
            None => false,
        }
    }

    /// Number of cached bitmaps.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is cached.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Decoded bytes currently held.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// The configured cap.
    #[cfg(test)]
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// Changes the cap, evicting down to it immediately.
    #[cfg(test)]
    pub fn set_max_bytes(&mut self, max_bytes: u64) {
        self.max_bytes = max_bytes;
        self.trim();
    }

    /// Evicts LRU entries until the cap holds, keeping at least one.
    #[cfg(test)]
    pub fn trim(&mut self) {
        while self.bytes > self.max_bytes && self.entries.len() > 1 {
            let Some(victim) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(id, _)| *id)
            else {
                break;
            };
            self.remove(victim);
        }
    }
}

fn byte_size(image: &ImageBuffer) -> u64 {
    (image.width() as u64) * (image.height() as u64) * ImageBuffer::BYTES_PER_PIXEL as u64
}

#[cfg(test)]
mod tests {
    use super::DecodeCache;
    use rizref_codecs::ImageBuffer;
    use rizref_core::AssetId;
    use std::sync::Arc;

    fn id(raw: u8) -> AssetId {
        AssetId::from_checksum([raw; 32])
    }

    fn image(side: u32) -> Arc<ImageBuffer> {
        Arc::new(ImageBuffer::filled(side, side, [1, 2, 3, 255]))
    }

    #[test]
    fn inserting_and_getting_refreshes_recency() {
        // 4 x 4 x 4 = 64 bytes each; the cap holds two.
        let mut cache = DecodeCache::new(128);
        assert_eq!(cache.max_bytes(), 128);
        cache.insert(id(1), image(4));
        cache.insert(id(2), image(4));

        assert!(cache.get(id(1)).is_some());
        let evicted = cache.insert(id(3), image(4));

        assert_eq!(evicted, vec![id(2)], "the least recently used goes first");
        assert!(cache.contains(id(1)));
        assert!(cache.contains(id(3)));
        assert_eq!(cache.bytes(), 128);
    }

    #[test]
    fn reinserting_the_same_asset_replaces_its_bytes() {
        let mut cache = DecodeCache::new(1_000);
        cache.insert(id(1), image(4));
        cache.insert(id(1), image(8));

        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes(), 8 * 8 * 4);
    }

    #[test]
    fn a_single_oversized_image_is_kept() {
        let mut cache = DecodeCache::new(10);

        let evicted = cache.insert(id(1), image(4));

        assert!(evicted.is_empty());
        assert!(cache.contains(id(1)));
        assert_eq!(cache.bytes(), 64);
    }

    #[test]
    fn lowering_the_cap_evicts_immediately() {
        let mut cache = DecodeCache::new(1_000);
        for raw in 0..4 {
            cache.insert(id(raw), image(4));
        }

        cache.set_max_bytes(64);

        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes(), 64);
    }

    #[test]
    fn removing_releases_the_byte_count() {
        let mut cache = DecodeCache::new(1_000);
        cache.insert(id(1), image(4));

        assert!(cache.remove(id(1)));
        assert!(!cache.remove(id(1)));
        assert_eq!(cache.bytes(), 0);
        assert!(cache.is_empty());
    }
}
