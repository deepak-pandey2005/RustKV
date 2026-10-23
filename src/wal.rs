//! Append-Only Write-Ahead Log (WAL) and Recovery Engine.
//!
//! Handles append-only persistence, binary record framing, crash recovery,
//! corruption detection, and disk synchronization.

use crate::error::{KvError, Result};
use crate::index::{Index, IndexEntry};
use crate::record::{HEADER_SIZE, Record, RecordHeader, RecordType};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

/// Recovery policy for opening database files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryMode {
    /// Safe recovery: automatically truncate an incomplete trailing record caused
    /// by an abrupt crash/power-loss, but fail immediately on mid-log corruption.
    TruncateTrailingIncomplete,
    /// Strict recovery: fail on any incomplete record or checksum error.
    Strict,
}

/// Statistics and metadata gathered during log recovery.
#[derive(Debug, Clone, Default)]
pub struct RecoveryReport {
    /// Total number of valid records replayed from the log.
    pub valid_records: usize,
    /// Number of set/update operations recovered.
    pub set_count: usize,
    /// Number of delete operations recovered.
    pub delete_count: usize,
    /// Number of incomplete trailing bytes truncated during recovery (if any).
    pub truncated_bytes: u64,
    /// Total valid byte length of the log file after recovery.
    pub valid_log_size: u64,
}

/// The append-only Write-Ahead Log file manager.
#[derive(Debug)]
pub struct Wal {
    path: PathBuf,
    file: File,
    write_offset: u64,
}

impl Wal {
    /// Opens the log file at the given path, running crash recovery to reconstruct the index.
    pub fn open(
        path: impl AsRef<Path>,
        mode: RecoveryMode,
    ) -> Result<(Self, Index, RecoveryReport)> {
        let path = path.as_ref().to_path_buf();

        // Open in read/write/create mode
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;

        let mut index = Index::new();
        let report = Self::recover(&mut file, &mut index, mode, &path)?;

        // Ensure file pointer is positioned at the end for appending
        file.seek(SeekFrom::Start(report.valid_log_size))?;

        let wal = Self {
            path,
            file,
            write_offset: report.valid_log_size,
        };

        Ok((wal, index, report))
    }

    /// Replays the log file from beginning to end to reconstruct the in-memory index.
    fn recover(
        file: &mut File,
        index: &mut Index,
        mode: RecoveryMode,
        _path: &Path,
    ) -> Result<RecoveryReport> {
        let total_file_len = file.metadata()?.len();
        file.seek(SeekFrom::Start(0))?;

        let mut offset = 0u64;
        let mut report = RecoveryReport::default();
        let mut header_buf = [0u8; HEADER_SIZE];

        while offset < total_file_len {
            let bytes_left = total_file_len - offset;

            // Check if there are fewer bytes left than a single record header
            if bytes_left < HEADER_SIZE as u64 {
                if mode == RecoveryMode::TruncateTrailingIncomplete {
                    // Safe crash recovery: truncate incomplete trailing header
                    file.set_len(offset)?;
                    report.truncated_bytes = bytes_left;
                    break;
                } else {
                    return Err(KvError::IncompleteRecord {
                        offset,
                        expected: HEADER_SIZE,
                        available: bytes_left as usize,
                    });
                }
            }

            // Read fixed-size header
            file.read_exact(&mut header_buf)?;

            // Decode header
            let header = match RecordHeader::decode(&header_buf) {
                Ok(hdr) => hdr,
                Err(err) => {
                    return Err(KvError::LogCorrupted {
                        offset,
                        reason: format!("Corrupt record header at offset {}: {}", offset, err),
                    });
                }
            };

            let payload_len = (header.key_len as u64) + (header.val_len as u64);
            let expected_record_len = (HEADER_SIZE as u64) + payload_len;

            // Check if the entire record fits within the remaining file bytes
            if bytes_left < expected_record_len {
                if mode == RecoveryMode::TruncateTrailingIncomplete {
                    // Incomplete trailing payload due to crash mid-write
                    file.set_len(offset)?;
                    report.truncated_bytes = bytes_left;
                    break;
                } else {
                    return Err(KvError::IncompleteRecord {
                        offset,
                        expected: expected_record_len as usize,
                        available: bytes_left as usize,
                    });
                }
            }

            // Read key
            let mut key = vec![0u8; header.key_len as usize];
            file.read_exact(&mut key)?;

            // Read value
            let mut value = vec![0u8; header.val_len as usize];
            file.read_exact(&mut value)?;

            // Verify CRC-32 checksum
            if let Err(err) = Record::verify_checksum(&header, &key, &value, offset) {
                // If checksum fails:
                // Is this strictly the final record at the very end of the file?
                let is_final_record = (offset + expected_record_len) == total_file_len;
                if is_final_record && mode == RecoveryMode::TruncateTrailingIncomplete {
                    file.set_len(offset)?;
                    report.truncated_bytes = bytes_left;
                    break;
                } else {
                    return Err(KvError::LogCorrupted {
                        offset,
                        reason: format!(
                            "Checksum mismatch for record at offset {} (is_final={}): {}",
                            offset, is_final_record, err
                        ),
                    });
                }
            }

            // Apply record to in-memory index
            match header.record_type {
                RecordType::Set => {
                    let entry = IndexEntry {
                        record_offset: offset,
                        value_offset: offset + (HEADER_SIZE as u64) + (header.key_len as u64),
                        value_len: header.val_len,
                        timestamp: header.timestamp,
                    };
                    index.insert(key, entry);
                    report.set_count += 1;
                }
                RecordType::Delete => {
                    index.remove(&key);
                    report.delete_count += 1;
                }
            }

            report.valid_records += 1;
            offset += expected_record_len;
        }

        report.valid_log_size = offset;
        Ok(report)
    }

    /// Appends a serialized record to the end of the log file.
    /// Returns the record offset and value offset.
    pub fn append(&mut self, record: &Record) -> Result<(u64, u64)> {
        let bytes = record.serialize();
        let record_offset = self.write_offset;
        let value_offset = record_offset + (HEADER_SIZE as u64) + (record.key.len() as u64);

        // Write all bytes to the underlying file
        self.file.write_all(&bytes)?;
        self.write_offset += bytes.len() as u64;

        Ok((record_offset, value_offset))
    }

    /// Reads a value payload directly from disk at the specified `value_offset`.
    /// Uses Linux positional `read_exact_at` so the append write pointer is undisturbed.
    pub fn read_value_at(&self, value_offset: u64, value_len: u32) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; value_len as usize];
        self.file.read_exact_at(&mut buf, value_offset)?;
        Ok(buf)
    }

    /// Flushes user-space buffers to the operating system kernel.
    pub fn flush(&mut self) -> Result<()> {
        self.file.flush()?;
        Ok(())
    }

    /// Flushes operating system buffers to underlying physical storage (fdatasync).
    pub fn sync_data(&self) -> Result<()> {
        self.file.sync_data()?;
        Ok(())
    }

    /// Flushes operating system buffers and file metadata to physical storage (fsync).
    pub fn sync_all(&self) -> Result<()> {
        self.file.sync_all()?;
        Ok(())
    }

    /// Returns the current logical size of the log file in bytes.
    #[inline]
    pub fn size(&self) -> u64 {
        self.write_offset
    }

    /// Returns the path to this log file.
    #[inline]
    pub fn path(&self) -> &Path {
        &self.path
    }
}
