//! In-Memory Hash Index for RustKV.
//!
//! Maps database keys to their physical disk coordinates (`IndexEntry`).
//! Lookups (`GET` and `EXISTS`) execute in O(1) time without reading the entire log.

use std::collections::HashMap;

/// Metadata pointing to the location of a key's value on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexEntry {
    /// Absolute byte offset in the log file where the record header begins.
    pub record_offset: u64,
    /// Absolute byte offset where the value payload begins.
    pub value_offset: u64,
    /// Length of the value payload in bytes.
    pub value_len: u32,
    /// Unix timestamp in milliseconds when the record was written.
    pub timestamp: u64,
}

/// In-memory hash index holding all live database keys.
#[derive(Debug, Default, Clone)]
pub struct Index {
    entries: HashMap<Vec<u8>, IndexEntry>,
}

impl Index {
    /// Creates an empty in-memory index.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Inserts or updates an index entry for a key.
    pub fn insert(&mut self, key: Vec<u8>, entry: IndexEntry) -> Option<IndexEntry> {
        self.entries.insert(key, entry)
    }

    /// Retrieves an entry for a given key.
    pub fn get(&self, key: &[u8]) -> Option<&IndexEntry> {
        self.entries.get(key)
    }

    /// Removes a key from the index (e.g. during a DELETE operation).
    pub fn remove(&mut self, key: &[u8]) -> Option<IndexEntry> {
        self.entries.remove(key)
    }

    /// Checks if a key exists in the index.
    pub fn contains_key(&self, key: &[u8]) -> bool {
        self.entries.contains_key(key)
    }

    /// Returns the total number of live keys in the database.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the index contains no keys.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clears the index.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Scans keys, optionally filtering by prefix, and returns them in sorted order.
    pub fn scan(&self, prefix: Option<&[u8]>) -> Vec<Vec<u8>> {
        let mut keys: Vec<Vec<u8>> = match prefix {
            Some(pref) => self
                .entries
                .keys()
                .filter(|k| k.starts_with(pref))
                .cloned()
                .collect(),
            None => self.entries.keys().cloned().collect(),
        };

        keys.sort();
        keys
    }

    /// Sums the bytes of all currently active values (useful for stats and compaction decisions).
    pub fn total_active_value_bytes(&self) -> u64 {
        self.entries.values().map(|e| e.value_len as u64).sum()
    }

    /// Sums the bytes of all currently active keys.
    pub fn total_active_key_bytes(&self) -> u64 {
        self.entries.keys().map(|k| k.len() as u64).sum()
    }

    /// Returns an iterator over all key-entry pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&Vec<u8>, &IndexEntry)> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_insert_get_remove() {
        let mut idx = Index::new();
        let key = b"k1".to_vec();
        let entry = IndexEntry {
            record_offset: 0,
            value_offset: 26,
            value_len: 10,
            timestamp: 1000,
        };

        assert!(!idx.contains_key(&key));
        idx.insert(key.clone(), entry);
        assert!(idx.contains_key(&key));
        assert_eq!(idx.get(&key), Some(&entry));

        assert_eq!(idx.len(), 1);
        let removed = idx.remove(&key);
        assert_eq!(removed, Some(entry));
        assert!(!idx.contains_key(&key));
        assert_eq!(idx.len(), 0);
    }

    #[test]
    fn test_index_scan_prefix() {
        let mut idx = Index::new();
        let dummy = IndexEntry {
            record_offset: 0,
            value_offset: 0,
            value_len: 0,
            timestamp: 0,
        };

        idx.insert(b"user:1".to_vec(), dummy);
        idx.insert(b"user:2".to_vec(), dummy);
        idx.insert(b"order:1".to_vec(), dummy);
        idx.insert(b"user:3".to_vec(), dummy);

        let user_keys = idx.scan(Some(b"user:"));
        assert_eq!(
            user_keys,
            vec![b"user:1".to_vec(), b"user:2".to_vec(), b"user:3".to_vec()]
        );

        let all_keys = idx.scan(None);
        assert_eq!(all_keys.len(), 4);
    }
}
