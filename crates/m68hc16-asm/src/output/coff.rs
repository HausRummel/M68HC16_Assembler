//! Writer for MASM's COFF-style `.OBJ`.
//!
//! Layout: file header (20B) -> section table (nsec*40B) -> section data (4-byte
//! aligned, only for TEXT/DATA sections) -> symbol table (nsym*18B) -> string
//! table. The format was reverse-engineered from the golden `JTE.OBJ` (see the
//! `obj-format` project note); it is deterministic except the 4-byte timestamp.
//!
//! Sections come straight from the assembled image: one per contiguous "touched"
//! run of addresses (a gap left by `org` separates sections), flagged TEXT if it
//! holds any instruction, DATA if it holds data but no instruction, else BSS
//! (reserve-only — no bytes in the file). An always-present empty `.bss` is
//! section 0. Symbols are the 17-or-so section symbols (each with an aux record)
//! followed by every program symbol in source-definition order.

use std::collections::HashMap;

use crate::encoder::Elem;
use crate::symbols::{Kind, SymbolTable};

const MAGIC: u16 = 0x0330;
const F_FLAGS: u16 = 0x0204;
const STYP_TEXT: u32 = 0x20;
const STYP_DATA: u32 = 0x40;
const STYP_BSS: u32 = 0x80;
const C_STAT: u8 = 3;

struct Section {
    name: &'static str,
    vaddr: u32,
    size: u32,
    flags: u32,
    /// File bytes (empty for BSS).
    data: Vec<u8>,
}

/// Render the assembled image as a COFF `.OBJ`. `data` is the final (filled)
/// image, `spans` the per-item element map, `symbols` the symbol values/kinds and
/// `sym_order` their `(name, first-occurrence context)` in symbol-table order.
/// `timestamp` fills the COFF header's time field.
pub fn write_coff(
    data: &[(u32, u8)],
    spans: &[(u32, u32, Elem)],
    symbols: &SymbolTable,
    sym_order: &[(String, Option<Elem>)],
    asct: bool,
    timestamp: u32,
) -> Vec<u8> {
    let img: HashMap<u32, u8> = data.iter().copied().collect();
    let sections = build_sections(spans, &img, asct);

    // Section number (1-based) of the first section sharing each name -> the
    // scnum MASM gives every symbol/section of that name.
    let mut first_scnum: HashMap<&str, i16> = HashMap::new();
    for (i, s) in sections.iter().enumerate() {
        first_scnum.entry(s.name).or_insert((i + 1) as i16);
    }
    let asct_scnum = *first_scnum.get(".asct").unwrap_or(&2);
    // scnum of a relocatable symbol = its containing section's scnum. With `ASCT`
    // every program symbol lives in an `.asct` section (scnum 2); without it, the
    // symbol takes the scnum of whichever content section (`.bss`/`.text`/`.data`)
    // holds its address. (jte: all Rel -> 2, unchanged; BASE_RAM: all Rel -> .bss 1.)
    let rel_scnum = |value: u32| -> i16 {
        if asct {
            asct_scnum
        } else {
            sections
                .iter()
                .find(|s| s.vaddr <= value && value < s.vaddr + s.size)
                .and_then(|s| first_scnum.get(s.name).copied())
                .unwrap_or(1)
        }
    };

    // ---- file offsets ----
    let nsec = sections.len();
    let mut off = 20 + nsec * 40;
    let mut scnptr = vec![0u32; nsec];
    for (i, s) in sections.iter().enumerate() {
        if s.flags != STYP_BSS && !s.data.is_empty() {
            off = align4(off);
            scnptr[i] = off as u32;
            off += s.data.len();
        }
    }
    let symptr = align4(off);

    // ---- symbol table + string table ----
    // Section symbols (each + 1 aux), then program symbols in definition order.
    let mut syms: Vec<SymEntry> = Vec::new();
    let mut aux_reg: u32 = 0; // "first data-bearing section size" register
    for s in &sections {
        if aux_reg == 0 && s.flags != STYP_BSS {
            aux_reg = s.size;
        }
        let scnum = *first_scnum.get(s.name).unwrap();
        syms.push(SymEntry::new(s.name.to_string(), s.vaddr, scnum, 0, 1));
        syms.push(SymEntry::aux(aux_reg));
    }

    let start = HashMap::<u32, Elem>::from_iter(spans.iter().map(|&(a, _, k)| (a, k)));
    for (name, ctx) in sym_order {
        let (value, kind) = match symbols.get_full(name) {
            Some(v) => v,
            None => continue,
        };
        // The symbol type is the width of its first-occurrence context; a label
        // whose first occurrence is a bare-label line (ctx None) falls back to the
        // element actually at its address. (Equate-with-nonzero-type quirks aside.)
        let (scnum, typ) = match kind {
            Kind::Abs => (-1i16, ctx.map_or(0, elem_type)),
            Kind::Rel => (
                rel_scnum(value as u32),
                match ctx {
                    Some(e) => elem_type(*e),
                    None => start.get(&(value as u32)).copied().map_or(0, elem_type),
                },
            ),
        };
        syms.push(SymEntry::new(name.clone(), value as u32, scnum, typ, 0));
    }

    // Encode names: <=8 inline, else into the string table (4-byte length first).
    let mut strtab: Vec<u8> = vec![0, 0, 0, 0];
    let mut name_field = |name: &str| -> [u8; 8] {
        let mut f = [0u8; 8];
        if name.len() <= 8 {
            f[..name.len()].copy_from_slice(name.as_bytes());
        } else {
            let offset = strtab.len() as u32;
            strtab.extend_from_slice(name.as_bytes());
            strtab.push(0);
            f[4..8].copy_from_slice(&offset.to_le_bytes());
        }
        f
    };

    let mut symbytes: Vec<u8> = Vec::with_capacity(syms.len() * 18);
    for e in &syms {
        if e.is_aux {
            symbytes.extend_from_slice(&e.value.to_le_bytes()); // aux length
            symbytes.extend_from_slice(&[0u8; 14]);
        } else {
            symbytes.extend_from_slice(&name_field(&e.name));
            symbytes.extend_from_slice(&e.value.to_le_bytes());
            symbytes.extend_from_slice(&e.scnum.to_le_bytes());
            symbytes.extend_from_slice(&e.typ.to_le_bytes());
            symbytes.push(C_STAT);
            symbytes.push(e.naux);
        }
    }
    let strtab_len = strtab.len() as u32;
    strtab[..4].copy_from_slice(&strtab_len.to_le_bytes());

    // ---- assemble the file ----
    let mut out: Vec<u8> = Vec::with_capacity(symptr + symbytes.len() + strtab.len());
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&(nsec as u16).to_le_bytes());
    out.extend_from_slice(&timestamp.to_le_bytes());
    out.extend_from_slice(&(symptr as u32).to_le_bytes());
    out.extend_from_slice(&(syms.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // optional header size
    out.extend_from_slice(&F_FLAGS.to_le_bytes());

    for (i, s) in sections.iter().enumerate() {
        let mut nm = [0u8; 8];
        nm[..s.name.len()].copy_from_slice(s.name.as_bytes());
        out.extend_from_slice(&nm);
        // BSS sections carry paddr=0; TEXT/DATA set paddr == vaddr.
        let paddr = if s.flags == STYP_BSS { 0 } else { s.vaddr };
        out.extend_from_slice(&paddr.to_le_bytes());
        out.extend_from_slice(&s.vaddr.to_le_bytes());
        out.extend_from_slice(&s.size.to_le_bytes());
        out.extend_from_slice(&scnptr[i].to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // relptr
        out.extend_from_slice(&0u32.to_le_bytes()); // lnnoptr
        out.extend_from_slice(&0u16.to_le_bytes()); // nreloc
        out.extend_from_slice(&0u16.to_le_bytes()); // nlnno
        out.extend_from_slice(&s.flags.to_le_bytes());
    }

    for (i, s) in sections.iter().enumerate() {
        if scnptr[i] != 0 {
            pad_align(&mut out, scnptr[i] as usize);
            out.extend_from_slice(&s.data);
        }
    }
    pad_align(&mut out, symptr);
    out.extend_from_slice(&symbytes);
    out.extend_from_slice(&strtab);
    out
}

struct SymEntry {
    name: String,
    value: u32,
    scnum: i16,
    typ: u16,
    naux: u8,
    is_aux: bool,
}

impl SymEntry {
    fn new(name: String, value: u32, scnum: i16, typ: u16, naux: u8) -> SymEntry {
        SymEntry { name, value, scnum, typ, naux, is_aux: false }
    }
    fn aux(length: u32) -> SymEntry {
        SymEntry { name: String::new(), value: length, scnum: 0, typ: 0, naux: 0, is_aux: true }
    }
}

fn elem_type(e: Elem) -> u16 {
    match e {
        Elem::Word => 3,
        Elem::Byte | Elem::Reserve => 2,
        Elem::Code => 0,
    }
}

fn align4(n: usize) -> usize {
    (n + 3) & !3
}

/// Pad `out` up to `target` for COFF's 4-byte section-data file alignment. MASM
/// does not zero-fill the gap: it leaves a `nop` word (0x274C) there, byte-swapped
/// like all section data (-> bytes 0x4C, 0x27). Section data is always even-length,
/// so the gap is 0 or 2 bytes and starts on a word boundary; emit the swapped-`nop`
/// pattern by file-offset parity to reproduce the gold image byte-for-byte.
fn pad_align(out: &mut Vec<u8>, target: usize) {
    while out.len() < target {
        out.push(if out.len() % 2 == 0 { 0x4C } else { 0x27 });
    }
}

#[cfg(test)]
mod tests {
    use crate::encoder::assemble_source;

    /// Find a program symbol's `(value, scnum, type)` by name in a COFF image.
    fn find_sym(obj: &[u8], want: &str) -> Option<(u32, i16, u16)> {
        let u32at = |o: usize| u32::from_le_bytes(obj[o..o + 4].try_into().unwrap());
        let symptr = u32at(8) as usize;
        let nsym = u32at(12) as usize;
        let strtab = symptr + nsym * 18;
        let mut i = 0;
        while i < nsym {
            let o = symptr + i * 18;
            let naux = obj[o + 17];
            let name = if u32at(o) == 0 {
                let so = strtab + u32at(o + 4) as usize;
                let end = obj[so..].iter().position(|&b| b == 0).unwrap() + so;
                String::from_utf8_lossy(&obj[so..end]).into_owned()
            } else {
                let end = obj[o..o + 8].iter().position(|&b| b == 0).unwrap_or(8);
                String::from_utf8_lossy(&obj[o..o + end]).into_owned()
            };
            if name == want {
                let value = u32at(o + 8);
                let scnum = i16::from_le_bytes([obj[o + 12], obj[o + 13]]);
                let typ = u16::from_le_bytes([obj[o + 14], obj[o + 15]]);
                return Some((value, scnum, typ));
            }
            i += 1 + naux as usize;
        }
        None
    }

    #[test]
    fn coff_structure_matches_oracle_rules() {
        // Mirrors the DOSBox-oracle probe: a code-bearing .asct section, with the
        // symbol type set by element width (code=0, FDB=word=3, FCB=byte=2).
        let src = "        ASCT\n\
                   \x20       ORG  $2000\n\
                   C1      nop\n\
                   \x20       rts\n\
                   F1      fdb  $1111\n\
                   B1      fcb  $33\n\
                   KONST   equ  $1234\n\
                   \x20       end\n";
        let obj = assemble_source(src);
        let bytes = super::write_coff(&obj.data, &obj.spans, &obj.symbols, &obj.sym_order, obj.asct, 0);

        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 0x0330, "COFF magic");
        assert_eq!(u16::from_le_bytes([bytes[18], bytes[19]]), 0x0204, "header flags");

        // Section 1 is the .asct region; it holds an instruction -> STYP_TEXT.
        let sec1 = 20 + 40; // section table entry [1]
        assert_eq!(&bytes[sec1..sec1 + 5], b".asct");
        let sflags = u32::from_le_bytes(bytes[sec1 + 36..sec1 + 40].try_into().unwrap());
        assert_eq!(sflags, 0x20, "code section is STYP_TEXT");

        // Section data is stored 16-bit byte-swapped (LE words) vs the image.
        let scnptr = u32::from_le_bytes(bytes[sec1 + 20..sec1 + 24].try_into().unwrap()) as usize;
        assert_eq!(&bytes[scnptr..scnptr + 2], &[0x4c, 0x27], "nop 0x274C -> swapped 4C 27");

        // Symbol types follow element width; equate is absolute.
        assert_eq!(find_sym(&bytes, "C1").unwrap().2, 0, "code label type 0");
        assert_eq!(find_sym(&bytes, "F1").unwrap().2, 3, "FDB label type 3 (word)");
        assert_eq!(find_sym(&bytes, "B1").unwrap().2, 2, "FCB label type 2 (byte)");
        assert_eq!(find_sym(&bytes, "KONST").unwrap(), (0x1234, -1, 0), "abs equate");
    }
}

/// `(name, vaddr)` of each section, for the listing's symbol-table section rows.
pub fn section_list(data: &[(u32, u8)], spans: &[(u32, u32, Elem)], asct: bool) -> Vec<(&'static str, u32)> {
    let img: HashMap<u32, u8> = data.iter().copied().collect();
    build_sections(spans, &img, asct).into_iter().map(|s| (s.name, s.vaddr)).collect()
}

/// Partition the spans into contiguous sections (gaps separate them) and slice
/// the filled image for each non-BSS section. Prepends the always-present empty
/// `.bss` section 0.
fn build_sections(spans: &[(u32, u32, Elem)], img: &HashMap<u32, u8>, asct: bool) -> Vec<Section> {
    // With `ASCT`, MASM's default `.bss` section sits empty at section 0 (all real
    // content was redirected to `.asct`); without it the real regions ARE the
    // sections, so there is no leading empty one.
    let mut secs = Vec::new();
    if asct {
        secs.push(Section { name: ".bss", vaddr: 0, size: 0, flags: STYP_BSS, data: Vec::new() });
    }
    if spans.is_empty() {
        return secs;
    }
    let mut sp: Vec<(u32, u32, Elem)> = spans.to_vec();
    sp.sort_by_key(|&(a, _, _)| a);

    let mut i = 0;
    while i < sp.len() {
        let vaddr = sp[i].0;
        let mut end = sp[i].0 + sp[i].1; // exclusive
        let mut has_code = false;
        let mut has_data = false;
        let mut j = i;
        while j < sp.len() && sp[j].0 <= end {
            end = end.max(sp[j].0 + sp[j].1);
            match sp[j].2 {
                Elem::Code => has_code = true,
                Elem::Word | Elem::Byte => has_data = true,
                Elem::Reserve => {}
            }
            j += 1;
        }
        let (flags, size, data);
        if has_code || has_data {
            flags = if has_code { STYP_TEXT } else { STYP_DATA };
            let padded = end + (end & 1); // TEXT/DATA size is even-padded
            size = padded - vaddr;
            let mut bytes: Vec<u8> = (vaddr..vaddr + size).map(|a| *img.get(&a).unwrap_or(&0xFF)).collect();
            // COFF stores 16-bit words little-endian; the HC16 image is big-endian
            // (as in the S-record), so HEX.exe swaps each word. Swap them here.
            for w in bytes.chunks_exact_mut(2) {
                w.swap(0, 1);
            }
            data = bytes;
        } else {
            flags = STYP_BSS;
            size = end - vaddr; // reserve extent, not padded
            data = Vec::new();
        }
        // With `ASCT` every region is the single absolute section `.asct`; without
        // it the region is named by its content (matches the BASE_RAM oracle: pure
        // reserve -> `.bss`).
        let name = if asct {
            ".asct"
        } else {
            match flags {
                STYP_TEXT => ".text",
                STYP_DATA => ".data",
                _ => ".bss",
            }
        };
        secs.push(Section { name, vaddr, size, flags, data });
        i = j;
    }
    secs
}
