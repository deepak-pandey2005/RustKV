//! The Core Storage Engine (KvEngine).
//!
//! Coordinates the append-only log (`Wal`), the in-memory hash index (`Index`),
//! durability policies, and log compaction.

use crate::error::Result;
use crate::index::{Index, IndexEntry};
use crate::record::{HEADER_SIZE, Record};
use crate::wal::{RecoveryMode, RecoveryReport, Wal};
use std::fs;
use std::path::{Path, PathBuf};

/// Policy governing when in-memory writes are synchronized to physical disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncPolicy {
    /// Synchronize after every single mutation (`fdatasync`).
    /// Highest durability; protects against sudden operating system crash and power loss.
    Always,
    /// Never explicitly sync; rely on operating system background page-cache writeback.
    /// Highest throughput; data is safe from application crash, but vulnerable to power loss.
    Never,
    /// Synchronize once every N write operations.
    EveryN(usize),
}

/// Configuration settings for the database engine.
#[derive(Debug, Clone)]
pub struct Config {
    /// Path to the data file.
    pub path: PathBuf,
    /// Durability synchronization policy.
    pub sync_policy: SyncPolicy,
    /// Crash recovery mode.
    pub recovery_mode: RecoveryMode,
}

impl Config {
    /// Creates a configuration with safe defaults (Always sync, safe recovery).
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            sync_policy: SyncPolicy::Always,
            recovery_mode: RecoveryMode::TruncateTrailingIncomplete,
        }
    }

    /// Sets the durability synchronization policy.
    pub fn with_sync_policy(mut self, policy: SyncPolicy) -> Self {
        self.sync_policy = policy;
        self
    }

    /// Sets the recovery mode.
    pub fn with_recovery_mode(mut self, mode: RecoveryMode) -> Self {
        self.recovery_mode = mode;
        self
    }
}

/// Statistics about the database storage engine.
#[derive(Debug, Clone)]
pub struct EngineStats {
    /// Total number of active keys.
    pub active_keys: usize,
    /// Current log file size in bytes on disk.
    pub log_file_size: u64,
    /// Estimated size of all active keys in bytes.
    pub active_key_bytes: u64,
    /// Estimated size of all active values in bytes.
    pub active_value_bytes: u64,
    /// Number of operations since last disk sync.
    pub uncommitted_ops: usize,
    /// Active durability sync policy.
    pub sync_policy: SyncPolicy,
}

/// Statistics reported after completing a log compaction.
#[derive(Debug, Clone)]
pub struct CompactionStats {
    /// Size of log file before compaction in bytes.
    pub old_log_size: u64,
    /// Size of log file after compaction in bytes.
    pub new_log_size: u64,
    /// Dead space reclaimed in bytes.
    pub reclaimed_bytes: u64,
    /// Total active keys preserved.
    pub keys_preserved: usize,
}

/// The persistent key-value storage engine.
#[derive(Debug)]
pub struct KvEngine {
    config: Config,
    wal: Wal,
    index: Index,
    ops_since_sync: usize,
    last_recovery_report: RecoveryReport,
}

impl KvEngine {
    /// Opens or creates a database using default configuration at the given file path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let config = Config::new(path);
        Self::open_with_config(config)
    }

    /// Opens or creates a database with the specified custom configuration.
    pub fn open_with_config(config: Config) -> Result<Self> {
        // Ensure parent directories exist
        if let Some(parent) = config.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }

        let (wal, index, report) = Wal::open(&config.path, config.recovery_mode)?;

        Ok(Self {
            config,
            wal,
            index,
            ops_since_sync: 0,
            last_recovery_report: report,
        })
    }

    /// Inserts or updates a key-value pair.
    pub fn set(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        let record = Record::new_set(key.to_vec(), value.to_vec())?;
        let (record_offset, value_offset) = self.wal.append(&record)?;

        // Update in-memory index
        let entry = IndexEntry {
            record_offset,
            value_offset,
            value_len: value.len() as u32,
            timestamp: record.timestamp,
        };
        self.index.insert(key.to_vec(), entry);

        self.handle_sync_policy()?;
        Ok(())
    }

    /// Retrieves the value associated with a key, if it exists.
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let entry = match self.index.get(key) {
            Some(entry) => *entry,
            None => return Ok(None),
        };

        // Positional read directly from the log file
        let value = self
            .wal
            .read_value_at(entry.value_offset, entry.value_len)?;
        Ok(Some(value))
    }

    /// Deletes a key from the database.
    /// Returns `true` if the key existed and was deleted, `false` otherwise.
    pub fn delete(&mut self, key: &[u8]) -> Result<bool> {
        if !self.index.contains_key(key) {
            return Ok(false);
        }

        // Write tombstone record
        let record = Record::new_delete(key.to_vec())?;
        self.wal.append(&record)?;

        // Remove from index
        self.index.remove(key);

        self.handle_sync_policy()?;
        Ok(true)
    }

    /// Checks if a key exists in the database without reading its value from disk.
    #[inline]
    pub fn exists(&self, key: &[u8]) -> bool {
        self.index.contains_key(key)
    }

    /// Lists database keys in sorted order, optionally filtered by a prefix.
    pub fn scan(&self, prefix: Option<&[u8]>) -> Vec<Vec<u8>> {
        self.index.scan(prefix)
    }

    /// Explicitly flushes buffers and syncs the log file to physical storage.
    pub fn sync(&mut self) -> Result<()> {
        self.wal.flush()?;
        self.wal.sync_data()?;
        self.ops_since_sync = 0;
        Ok(())
    }

    /// Handles durability synchronization according to `SyncPolicy`.
    fn handle_sync_policy(&mut self) -> Result<()> {
        self.ops_since_sync += 1;
        match self.config.sync_policy {
            SyncPolicy::Always => {
                self.wal.flush()?;
                self.wal.sync_data()?;
                self.ops_since_sync = 0;
            }
            SyncPolicy::EveryN(n) => {
                if self.ops_since_sync >= n {
                    self.wal.flush()?;
                    self.wal.sync_data()?;
                    self.ops_since_sync = 0;
                }
            }
            SyncPolicy::Never => {
                // Let OS kernel buffer and flush periodically
            }
        }
        Ok(())
    }

    /// Compacts the log file by creating a new clean log containing only the latest
    /// active records, reclaiming dead space from overwrites and deletions.
    pub fn compact(&mut self) -> Result<CompactionStats> {
        let old_log_size = self.wal.size();
        let compact_path = self.config.path.with_extension("compact");

        // Step 1: Open temporary compaction file
        let mut compact_file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&compact_path)?;

        let mut new_index = Index::new();
        let mut write_offset = 0u64;

        // Step 2: Iterate over all live keys in the current index
        for (key, entry) in self.index.iter() {
            let val = self
                .wal
                .read_value_at(entry.value_offset, entry.value_len)?;
            let record = Record::with_type_and_time(
                crate::record::RecordType::Set,
                key.clone(),
                val,
                entry.timestamp,
            )?;

            let bytes = record.serialize();
            use std::io::Write;
            compact_file.write_all(&bytes)?;

            let new_entry = IndexEntry {
                record_offset: write_offset,
                value_offset: write_offset + (HEADER_SIZE as u64) + (key.len() as u64),
                value_len: entry.value_len,
                timestamp: entry.timestamp,
            };
            new_index.insert(key.clone(), new_entry);
            write_offset += bytes.len() as u64;
        }

        // Step 3: Fsync the compacted file to ensure physical persistence
        compact_file.sync_all()?;
        drop(compact_file);

        // Step 4: Atomically replace old log file with compacted file
        fs::rename(&compact_path, &self.config.path)?;

        // Step 5: Reopen Wal on the newly compacted file
        let (new_wal, _, _) = Wal::open(&self.config.path, self.config.recovery_mode)?;
        let new_log_size = new_wal.size();

        self.wal = new_wal;
        self.index = new_index;
        self.ops_since_sync = 0;

        let reclaimed_bytes = old_log_size.saturating_sub(new_log_size);

        Ok(CompactionStats {
            old_log_size,
            new_log_size,
            reclaimed_bytes,
            keys_preserved: self.index.len(),
        })
    }

    /// Returns storage engine diagnostics and statistics.
    pub fn stats(&self) -> EngineStats {
        EngineStats {
            active_keys: self.index.len(),
            log_file_size: self.wal.size(),
            active_key_bytes: self.index.total_active_key_bytes(),
            active_value_bytes: self.index.total_active_value_bytes(),
            uncommitted_ops: self.ops_since_sync,
            sync_policy: self.config.sync_policy,
        }
    }

    /// Returns the initial recovery report generated during startup.
    pub fn recovery_report(&self) -> &RecoveryReport {
        &self.last_recovery_report
    }

    /// Returns the data path.
    pub fn path(&self) -> &Path {
        &self.config.path
    }
}
