//! IEEE 802.3 CRC-32 Checksum Implementation.
//!
//! This module provides a fast, zero-dependency CRC-32 implementation
//! using a compile-time generated lookup table (polynomial `0xEDB88320`).
//! It matches the CRC-32 standard used by gzip, Ethernet, and PNG.

/// Precomputed CRC-32 lookup table generated at compile time.
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let poly = 0xEDB88320u32;
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ poly;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

/// Computes the CRC-32 checksum of a byte slice.
#[inline]
pub fn crc32(data: &[u8]) -> u32 {
    let mut hasher = Crc32Hasher::new();
    hasher.update(data);
    hasher.finalize()
}

/// Incremental CRC-32 hasher.
#[derive(Debug, Clone, Copy)]
pub struct Crc32Hasher {
    state: u32,
}

impl Default for Crc32Hasher {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc32Hasher {
    /// Creates a new CRC-32 hasher with the initial seed.
    #[inline]
    pub const fn new() -> Self {
        Self { state: 0xFFFF_FFFF }
    }

    /// Updates the hasher with a byte slice.
    #[inline]
    pub fn update(&mut self, data: &[u8]) {
        for &byte in data {
            let index = ((self.state ^ (byte as u32)) & 0xFF) as usize;
            self.state = (self.state >> 8) ^ CRC32_TABLE[index];
        }
    }

    /// Finalizes the checksum and returns the 32-bit result.
    #[inline]
    pub const fn finalize(&self) -> u32 {
        self.state ^ 0xFFFF_FFFF
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_slice() {
        assert_eq!(crc32(&[]), 0x0000_0000);
    }

    #[test]
    fn test_standard_test_vector() {
        // Standard ASCII test vector "123456789"
        let data = b"123456789";
        assert_eq!(crc32(data), 0xCBF4_3926);
    }

    #[test]
    fn test_incremental_updates() {
        let mut hasher = Crc32Hasher::new();
        hasher.update(b"1234");
        hasher.update(b"56789");
        assert_eq!(hasher.finalize(), 0xCBF4_3926);
    }

    #[test]
    fn test_single_byte_mutation_changes_hash() {
        let mut data = b"hello world from rustkv".to_vec();
        let hash1 = crc32(&data);
        data[0] ^= 0x01;
        let hash2 = crc32(&data);
        assert_ne!(hash1, hash2);
    }
}
