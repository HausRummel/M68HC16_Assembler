//! Flat binary-image writer.
//!
//! The `.S19` encodes the assembled image as address-tagged records; this writes
//! the same bytes as a raw binary — the contiguous image from the lowest to the
//! highest emitted address, with any gaps (between sections, or reserved holes)
//! filled with `0xFF`, the flash-erase value the section fill already uses. The
//! returned base address is the image's load offset (the lowest emitted address);
//! an image with no emitted bytes (e.g. a reserve- or equate-only module) yields an
//! empty file.

/// Fill byte for addresses inside the image extent that nothing emitted — `0xFF`,
/// matching erased flash and the intra-section fill in `fill_sections`.
const FILL: u8 = 0xFF;

/// Render `(address, byte)` data as a flat binary image. Returns `(base, bytes)`
/// where `base` is the lowest emitted address; `bytes[i]` is the byte at
/// `base + i`. Empty input yields `(0, [])`.
pub fn write_binary(data: &[(u32, u8)]) -> (u32, Vec<u8>) {
    let (Some(min), Some(max)) = (
        data.iter().map(|&(a, _)| a).min(),
        data.iter().map(|&(a, _)| a).max(),
    ) else {
        return (0, Vec::new());
    };
    let len = (max - min + 1) as usize;
    let mut img = vec![FILL; len];
    for &(addr, byte) in data {
        img[(addr - min) as usize] = byte;
    }
    (min, img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(write_binary(&[]), (0, Vec::new()));
    }

    #[test]
    fn fills_gaps_with_ff_and_reports_base() {
        // Bytes at 0x2000, 0x2001, then a gap, then 0x2004.
        let data = [(0x2000, 0x12), (0x2001, 0x34), (0x2004, 0x56)];
        let (base, img) = write_binary(&data);
        assert_eq!(base, 0x2000);
        assert_eq!(img, vec![0x12, 0x34, 0xFF, 0xFF, 0x56]);
    }

    #[test]
    fn base_is_lowest_address_not_zero() {
        let (base, img) = write_binary(&[(0x10000, 0xAB)]);
        assert_eq!(base, 0x10000);
        assert_eq!(img, vec![0xAB]);
    }
}
