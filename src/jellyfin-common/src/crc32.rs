//! CRC-32/ISO-HDLC checksums.

const POLYNOMIAL: u32 = 0xedb8_8320;
const TABLE: [u32; 256] = make_table();

const fn make_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut index = 0;

    while index < table.len() {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            let mask = 0_u32.wrapping_sub(value & 1);
            value = (value >> 1) ^ (POLYNOMIAL & mask);
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }

    table
}

/// Computes the CRC-32/ISO-HDLC checksum used by Jellyfin.
///
/// This is the reflected CRC-32 variant with polynomial `0xEDB88320`, an
/// initial value of `0xFFFFFFFF`, and a final XOR of `0xFFFFFFFF`.
#[must_use]
pub fn compute(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;

    for &byte in bytes {
        let index = usize::from((crc as u8) ^ byte);
        crc = (crc >> 8) ^ TABLE[index];
    }

    !crc
}

/// Namespace-compatible entry point for Jellyfin's `Crc32` utility.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Crc32;

impl Crc32 {
    /// Computes the CRC-32/ISO-HDLC checksum of `bytes`.
    #[must_use]
    pub fn compute(bytes: &[u8]) -> u32 {
        compute(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::{Crc32, compute};

    #[test]
    fn compute_empty_is_zero() {
        assert_eq!(0, Crc32::compute(&[]));
    }

    #[test]
    fn compute_text_matches_official_vector() {
        assert_eq!(
            0x414f_a339,
            compute(b"The quick brown fox jumps over the lazy dog")
        );
    }

    #[test]
    fn compute_binary_matches_official_vectors() {
        assert_eq!(0x190a_55ad, compute(&[0; 32]));
        assert_eq!(0xff6c_ab0b, compute(&[0xff; 32]));

        let ascending = std::array::from_fn::<_, 32, _>(|index| index as u8);
        assert_eq!(0x9126_7e8a, compute(&ascending));
    }

    #[test]
    fn compute_standard_check_value() {
        assert_eq!(0xcbf4_3926, compute(b"123456789"));
    }
}
