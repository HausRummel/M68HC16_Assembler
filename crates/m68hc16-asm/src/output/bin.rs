//! Flat binary-image writer.
//!
//! The `.S19` encodes the assembled image as address-tagged records; this writes
//! the same bytes as a raw burnable ROM image. The image spans whole 64 KB pages:
//! from the page containing the lowest emitted address down to its page base, up to
//! the page boundary above the highest emitted address. Gaps (between sections, or
//! reserved holes) and the trailing pad are filled with `0xFF`, the flash-erase
//! value the section fill already uses. (The CPU16 memory map is paged in 64 KB
//! banks — Page $0 = 0x00000-0x0FFFF, code Pages $1-$3 = 0x10000-0x3FFFF — so a ROM
//! image rounds out to page boundaries: code reaching into Page $3 yields a 0x40000
//! (256 KB) image, matching the reference ROM images the original toolchain
//! produced.) An image with no emitted bytes (a reserve- or equate-only module)
//! yields an empty file.

/// Fill byte for unwritten addresses inside the image extent — `0xFF`, matching
/// erased flash and the intra-section fill in `fill_sections`.
const FILL: u8 = 0xFF;

/// CPU16 bank/page size; the binary image is rounded out to whole pages.
const PAGE: u32 = 0x10000;

/// Render `(address, byte)` data as a flat ROM image rounded to 64 KB pages.
/// Returns `(base, bytes)` where `base` is the page-aligned load address and
/// `bytes[i]` is the byte at `base + i`. Empty input yields `(0, [])`.
pub fn write_binary(data: &[(u32, u8)]) -> (u32, Vec<u8>) {
    let (Some(min), Some(max)) = (
        data.iter().map(|&(a, _)| a).min(),
        data.iter().map(|&(a, _)| a).max(),
    ) else {
        return (0, Vec::new());
    };
    let base = (min / PAGE) * PAGE; // page floor
    let end = (max / PAGE + 1) * PAGE; // exclusive page ceiling
    let mut img = vec![FILL; (end - base) as usize];
    for &(addr, byte) in data {
        img[(addr - base) as usize] = byte;
    }
    (base, img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(write_binary(&[]), (0, Vec::new()));
    }

    #[test]
    fn rounds_to_page_base_zero_and_fills_ff() {
        // Bytes in Page $0: 0x2000, 0x2001, a gap, then 0x2004.
        let data = [(0x2000, 0x12), (0x2001, 0x34), (0x2004, 0x56)];
        let (base, img) = write_binary(&data);
        assert_eq!(base, 0); // page floor of 0x2000
        assert_eq!(img.len(), 0x10000); // one whole 64 KB page
        assert_eq!(&img[0x2000..0x2005], &[0x12, 0x34, 0xFF, 0xFF, 0x56]);
        assert_eq!(img[0], 0xFF);
        assert_eq!(img[0xFFFF], 0xFF);
    }

    #[test]
    fn base_is_page_floor_extent_is_page_ceiling() {
        // Data spanning Pages $1-$3 -> base 0x10000, extent through 0x3FFFF.
        let (base, img) = write_binary(&[(0x10000, 0xAB), (0x37EB5, 0xCD)]);
        assert_eq!(base, 0x10000);
        assert_eq!(img.len(), 0x30000); // pages $1,$2,$3
        assert_eq!(img[0], 0xAB);
        assert_eq!(img[0x37EB5 - 0x10000], 0xCD);
        assert_eq!(*img.last().unwrap(), 0xFF);
    }

    #[test]
    fn jte_like_image_is_256k_from_zero() {
        // Vectors at 0 + data reaching into Page $3 -> a 0x40000 image at base 0,
        // matching the reference ROM-image extent.
        let (base, img) = write_binary(&[(0, 0x0F), (0x37EB5, 0x42)]);
        assert_eq!(base, 0);
        assert_eq!(img.len(), 0x40000);
    }
}
