//! Error types for RustKV.

use std::fmt;
use std::io;

/// Convenient type alias for Results returning `KvError`.
pub type Result<T> = std::result::Result<T, KvError>;

/// Represents errors that can occur during database operations.
#[derive(Debug)]
pub enum KvError {
    /// Standard I/O errors occurring during file operations.
    Io(io::Error),

    /// Attempted to retrieve or delete a key that does not exist.
    KeyNotFound(String),

    /// A key cannot be empty (0 bytes).
    EmptyKey,

    /// Key length exceeds the maximum allowed limit.
    KeyTooLarge { size: usize, max: usize },

    /// Value length exceeds the maximum allowed limit.
    ValueTooLarge { size: usize, max: usize },

    /// Record header or structure is invalid (e.g., bad magic bytes, unknown opcode).
    InvalidRecord(String),

    /// Record CRC-32 checksum did not match the expected value.
    ChecksumMismatch {
        offset: u64,
        expected: u32,
        actual: u32,
    },

    /// Corrupted record encountered mid-log where valid data was expected.
    LogCorrupted { offset: u64, reason: String },

    /// Trailing record is incomplete due to an unexpected EOF (e.g. crash during write).
    IncompleteRecord {
        offset: u64,
        expected: usize,
        available: usize,
    },

    /// Database instance is closed.
    DatabaseClosed,

    /// General internal error.
    Other(String),
}

impl fmt::Display for KvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {}", err),
            Self::KeyNotFound(key) => write!(f, "Key not found: '{}'", key),
            Self::EmptyKey => write!(f, "Key cannot be empty"),
            Self::KeyTooLarge { size, max } => {
                write!(
                    f,
                    "Key size ({} bytes) exceeds maximum ({} bytes)",
                    size, max
                )
            }
            Self::ValueTooLarge { size, max } => {
                write!(
                    f,
                    "Value size ({} bytes) exceeds maximum ({} bytes)",
                    size, max
                )
            }
            Self::InvalidRecord(msg) => write!(f, "Invalid record format: {}", msg),
            Self::ChecksumMismatch {
                offset,
                expected,
                actual,
            } => write!(
                f,
                "Checksum mismatch at offset {}: expected 0x{:08X}, calculated 0x{:08X}",
                offset, expected, actual
            ),
            Self::LogCorrupted { offset, reason } => {
                write!(
                    f,
                    "Log corruption detected at offset {}: {}",
                    offset, reason
                )
            }
            Self::IncompleteRecord {
                offset,
                expected,
                available,
            } => write!(
                f,
                "Incomplete record at offset {}: expected {} bytes, but only {} bytes available (EOF)",
                offset, expected, available
            ),
            Self::DatabaseClosed => write!(f, "Database is closed"),
            Self::Other(msg) => write!(f, "Database error: {}", msg),
        }
    }
}

impl std::error::Error for KvError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for KvError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}
