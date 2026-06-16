//! Output-file writers (S-record, raw binary, COFF object, listing, map).

pub mod bin;
pub mod coff;
pub mod listing;
pub mod map;
pub mod srec;

/// Re-encode a Latin-1 string (as produced by [`crate::encoder::decode_latin1`])
/// back to its original bytes. The `.LST` is a DOS text file whose source column
/// must reproduce the input bytes verbatim, so it is written through this rather
/// than UTF-8.
pub fn encode_latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u8).collect()
}
