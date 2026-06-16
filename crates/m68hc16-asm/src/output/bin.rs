//! Flat binary-image writer.
//!
//! The `.S19` encodes the assembled image as address-tagged records; this writes
//! the same bytes as a raw burnable ROM image over a fixed `[base, base+size)`
//! window, with every address nothing emitted into (gaps between sections, reserved
//! holes, the leading/trailing pad) set to `fill`. This mirrors the common
//! `srec_cat <s19> -fill 0xFF 0x00000 0x40000 -binary` flow: the target ROM is a
//! 0x40000 (256 KB) image at base 0 filled with 0xFF (flash-erase). `base`, `size`
//! and `fill` are all caller-chosen so the same writer serves other HC16 targets.

/// Render `(address, byte)` data as a flat binary image of exactly `size` bytes
/// starting at `base`, every unwritten byte set to `fill`. Returns the image and
/// the count of input bytes that fell OUTSIDE `[base, base+size)` (and were thus
/// dropped — a non-zero count means `size`/`base` are too small for the program).
pub fn write_binary(data: &[(u32, u8)], base: u32, size: u32, fill: u8) -> (Vec<u8>, usize) {
    let mut img = vec![fill; size as usize];
    let mut dropped = 0usize;
    for &(addr, byte) in data {
        // `addr - base` only when in range, computed to avoid u32 underflow/overflow.
        if addr >= base && addr - base < size {
            img[(addr - base) as usize] = byte;
        } else {
            dropped += 1;
        }
    }
    (img, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_data_is_all_fill() {
        let (img, dropped) = write_binary(&[], 0, 0x40000, 0xFF);
        assert_eq!(img.len(), 0x40000);
        assert!(img.iter().all(|&b| b == 0xFF));
        assert_eq!(dropped, 0);
    }

    #[test]
    fn places_bytes_and_fills_gaps() {
        // The default window: base 0, 256 KB, 0xFF fill.
        let data = [(0x2000, 0x12), (0x2001, 0x34), (0x2004, 0x56)];
        let (img, dropped) = write_binary(&data, 0, 0x40000, 0xFF);
        assert_eq!(img.len(), 0x40000);
        assert_eq!(&img[0x2000..0x2005], &[0x12, 0x34, 0xFF, 0xFF, 0x56]);
        assert_eq!(img[0], 0xFF);
        assert_eq!(*img.last().unwrap(), 0xFF);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn custom_base_size_and_fill() {
        // A small window based at 0x10000, zero-filled.
        let (img, dropped) = write_binary(&[(0x10000, 0xAB), (0x10FFF, 0xCD)], 0x10000, 0x1000, 0x00);
        assert_eq!(img.len(), 0x1000);
        assert_eq!(img[0], 0xAB);
        assert_eq!(img[0xFFF], 0xCD);
        assert_eq!(img[1], 0x00);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn out_of_window_bytes_are_dropped_and_counted() {
        let (img, dropped) = write_binary(&[(0x100, 0x11), (0x50000, 0x22)], 0, 0x40000, 0xFF);
        assert_eq!(img[0x100], 0x11);
        assert_eq!(dropped, 1); // 0x50000 is past the 0x40000 window
    }
}
