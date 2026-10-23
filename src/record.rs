//! Binary Record Format and Serialization for RustKV.
//!
//! Record Layout (Fixed 24-byte Header + Variable Payload):
//! ```text
//! +------------------+-----------------+---------------+----------------+
//! |  Magic Bytes (2) | Record Type (1) |   Flags (1)   | Timestamp (8)  |
//! +------------------+-----------------+---------------+----------------+
//! |   Key Len (4)    |   Val Len (4)   | Checksum (4)  | Key Payload... |
//! +------------------+-----------------+---------------+----------------+
//! | Value Payload...                                                    |
//! +--------------------------------------------------------------------+
//! ```
//! Total Header Size: 24 bytes.
//! Checksum covers: RecordType + Flags + Timestamp + KeyLen + ValLen + Key + Value.

use crate::crc::Crc32Hasher;
use crate::error::{KvError, Result};
use std::time::{SystemTime, UNIX_EPOCH};

/// Sentinel magic bytes prefixing every valid record: "RK" (0x52, 0x4B)
pub const MAGIC: [u8; 2] = [0x52, 0x4B];

/// Fixed header size in bytes.
pub const HEADER_SIZE: usize = 24;

/// Maximum allowable key size: 64 KiB.
pub const MAX_KEY_SIZE: usize = 64 * 1024;

/// Maximum allowable value size: 64 MiB.
pub const MAX_VALUE_SIZE: usize = 64 * 1024 * 1024;

/// Operation type indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RecordType {
    /// Insert or update a key with a value.
    Set = 0x01,
    /// Delete a key (tombstone record).
    Delete = 0x02,
}

impl RecordType {
    pub fn from_u8(val: u8) -> Result<Self> {
        match val {
            0x01 => Ok(Self::Set),
            0x02 => Ok(Self::Delete),
            other => Err(KvError::InvalidRecord(format!(
                "Unknown record opcode: 0x{:02X}",
                other
            ))),
        }
    }
}

/// Parsed header of a database record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordHeader {
    pub record_type: RecordType,
    pub flags: u8,
    pub timestamp: u64,
    pub key_len: u32,
    pub val_len: u32,
    pub checksum: u32,
}

impl RecordHeader {
    /// Decodes a 24-byte header from a slice.
    pub fn decode(bytes: &[u8; HEADER_SIZE]) -> Result<Self> {
        if bytes[0..2] != MAGIC {
            return Err(KvError::InvalidRecord(format!(
                "Invalid magic bytes: [0x{:02X}, 0x{:02X}] (expected [0x52, 0x4B])",
                bytes[0], bytes[1]
            )));
        }

        let record_type = RecordType::from_u8(bytes[2])?;
        let flags = bytes[3];
        let timestamp = u64::from_le_bytes(bytes[4..12].try_into().unwrap());
        let key_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let val_len = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
        let checksum = u32::from_le_bytes(bytes[20..24].try_into().unwrap());

        // Validate lengths before returning to avoid unbounded allocations
        if key_len as usize > MAX_KEY_SIZE {
            return Err(KvError::KeyTooLarge {
                size: key_len as usize,
                max: MAX_KEY_SIZE,
            });
        }

        if val_len as usize > MAX_VALUE_SIZE {
            return Err(KvError::ValueTooLarge {
                size: val_len as usize,
                max: MAX_VALUE_SIZE,
            });
        }

        if key_len == 0 {
            return Err(KvError::EmptyKey);
        }

        Ok(Self {
            record_type,
            flags,
            timestamp,
            key_len,
            val_len,
            checksum,
        })
    }

    /// Encodes the header into a 24-byte array given the precomputed checksum.
    pub fn encode(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0..2].copy_from_slice(&MAGIC);
        buf[2] = self.record_type as u8;
        buf[3] = self.flags;
        buf[4..12].copy_from_slice(&self.timestamp.to_le_bytes());
        buf[12..16].copy_from_slice(&self.key_len.to_le_bytes());
        buf[16..20].copy_from_slice(&self.val_len.to_le_bytes());
        buf[20..24].copy_from_slice(&self.checksum.to_le_bytes());
        buf
    }

    /// Returns the total size in bytes of the complete record on disk.
    #[inline]
    pub fn total_record_size(&self) -> usize {
        HEADER_SIZE + self.key_len as usize + self.val_len as usize
    }
}

/// A complete database record containing type, timestamp, key, and value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub record_type: RecordType,
    pub timestamp: u64,
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

impl Record {
    /// Creates a new `Set` record with the current timestamp.
    pub fn new_set(key: Vec<u8>, value: Vec<u8>) -> Result<Self> {
        Self::with_type_and_time(RecordType::Set, key, value, current_timestamp_ms())
    }

    /// Creates a new `Delete` (tombstone) record with the current timestamp.
    pub fn new_delete(key: Vec<u8>) -> Result<Self> {
        Self::with_type_and_time(RecordType::Delete, key, Vec::new(), current_timestamp_ms())
    }

    /// Creates a record with an explicit timestamp and type.
    pub fn with_type_and_time(
        record_type: RecordType,
        key: Vec<u8>,
        value: Vec<u8>,
        timestamp: u64,
    ) -> Result<Self> {
        if key.is_empty() {
            return Err(KvError::EmptyKey);
        }
        if key.len() > MAX_KEY_SIZE {
            return Err(KvError::KeyTooLarge {
                size: key.len(),
                max: MAX_KEY_SIZE,
            });
        }
        if value.len() > MAX_VALUE_SIZE {
            return Err(KvError::ValueTooLarge {
                size: value.len(),
                max: MAX_VALUE_SIZE,
            });
        }

        Ok(Self {
            record_type,
            timestamp,
            key,
            value,
        })
    }

    /// Calculates the CRC-32 checksum for this record.
    /// Checksum covers: `record_type || flags || timestamp || key_len || val_len || key || value`.
    pub fn compute_checksum(&self, flags: u8) -> u32 {
        let mut hasher = Crc32Hasher::new();
        hasher.update(&[self.record_type as u8, flags]);
        hasher.update(&self.timestamp.to_le_bytes());
        hasher.update(&(self.key.len() as u32).to_le_bytes());
        hasher.update(&(self.value.len() as u32).to_le_bytes());
        hasher.update(&self.key);
        hasher.update(&self.value);
        hasher.finalize()
    }

    /// Serializes the record to a binary buffer ready to be written to disk.
    pub fn serialize(&self) -> Vec<u8> {
        let flags = 0u8;
        let checksum = self.compute_checksum(flags);
        let header = RecordHeader {
            record_type: self.record_type,
            flags,
            timestamp: self.timestamp,
            key_len: self.key.len() as u32,
            val_len: self.value.len() as u32,
            checksum,
        };

        let mut buf = Vec::with_capacity(header.total_record_size());
        buf.extend_from_slice(&header.encode());
        buf.extend_from_slice(&self.key);
        buf.extend_from_slice(&self.value);
        buf
    }

    /// Validates payload against the header's expected checksum.
    pub fn verify_checksum(
        header: &RecordHeader,
        key: &[u8],
        value: &[u8],
        offset: u64,
    ) -> Result<()> {
        let mut hasher = Crc32Hasher::new();
        hasher.update(&[header.record_type as u8, header.flags]);
        hasher.update(&header.timestamp.to_le_bytes());
        hasher.update(&header.key_len.to_le_bytes());
        hasher.update(&header.val_len.to_le_bytes());
        hasher.update(key);
        hasher.update(value);
        let actual = hasher.finalize();

        if actual != header.checksum {
            return Err(KvError::ChecksumMismatch {
                offset,
                expected: header.checksum,
                actual,
            });
        }
        Ok(())
    }
}

/// Helper function to return current system time in milliseconds since UNIX epoch.
pub fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_serialize_deserialize_cycle() {
        let rec =
            Record::new_set(b"user:100".to_vec(), b"Alice Database Engineer".to_vec()).unwrap();
        let bytes = rec.serialize();

        assert_eq!(bytes.len(), HEADER_SIZE + rec.key.len() + rec.value.len());

        let mut header_bytes = [0u8; HEADER_SIZE];
        header_bytes.copy_from_slice(&bytes[..HEADER_SIZE]);
        let header = RecordHeader::decode(&header_bytes).unwrap();

        assert_eq!(header.record_type, RecordType::Set);
        assert_eq!(header.key_len as usize, rec.key.len());
        assert_eq!(header.val_len as usize, rec.value.len());

        let key = &bytes[HEADER_SIZE..HEADER_SIZE + header.key_len as usize];
        let val = &bytes[HEADER_SIZE + header.key_len as usize..];

        Record::verify_checksum(&header, key, val, 0).expect("Checksum must be valid");
        assert_eq!(key, rec.key.as_slice());
        assert_eq!(val, rec.value.as_slice());
    }

    #[test]
    fn test_corrupted_payload_checksum_fails() {
        let rec = Record::new_set(b"important_key".to_vec(), b"secure_value".to_vec()).unwrap();
        let mut bytes = rec.serialize();

        // Corrupt a byte in the value
        let last_idx = bytes.len() - 1;
        bytes[last_idx] ^= 0x55;

        let mut header_bytes = [0u8; HEADER_SIZE];
        header_bytes.copy_from_slice(&bytes[..HEADER_SIZE]);
        let header = RecordHeader::decode(&header_bytes).unwrap();

        let key = &bytes[HEADER_SIZE..HEADER_SIZE + header.key_len as usize];
        let val = &bytes[HEADER_SIZE + header.key_len as usize..];

        let err = Record::verify_checksum(&header, key, val, 100).unwrap_err();
        match err {
            KvError::ChecksumMismatch { offset, .. } => assert_eq!(offset, 100),
            _ => panic!("Expected ChecksumMismatch"),
        }
    }

    #[test]
    fn test_invalid_magic_bytes() {
        let mut header_bytes = [0u8; HEADER_SIZE];
        header_bytes[0] = 0xFF;
        header_bytes[1] = 0xFF;

        let err = RecordHeader::decode(&header_bytes).unwrap_err();
        match err {
            KvError::InvalidRecord(_) => {}
            _ => panic!("Expected InvalidRecord error"),
        }
    }

    #[test]
    fn test_empty_key_fails() {
        let res = Record::new_set(Vec::new(), b"val".to_vec());
        assert!(matches!(res, Err(KvError::EmptyKey)));
    }
}
