//! Checksum helpers.

use crc32fast::Hasher;

/// Compute CRC-32 (IEEE 802.3) of a byte slice.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut h = Hasher::new();
    h.update(bytes);
    h.finalize()
}
