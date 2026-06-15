//! MASM-faithful `.LST` assembly-listing writer.
//!
//! The listing has three parts: the source body (one row per source line), the
//! Symbol Table, and the Cross-Reference Table, paginated under a repeating page
//! header. This module is built incrementally; today it renders the **Symbol
//! Table** block, which is the byte-faithful name->address table other tooling
//! needs. Column layout was reverse-engineered from real MASM 4.6 output:
//!
//! ```text
//! symbol name       attrib.   section    value
//! -----------       -------   -------    -----
//! .asct             section   249        0x000f8000
//! BYTE1             .asct     249        0x0000200a
//! KONST             abs                  0x000000ff
//! ```
//!
//! Each row is `name  attrib  section  value` where the name field is min-16,
//! attrib min-8, section min-9, each followed by two spaces, then `0x` + 8 lower
//! hex digits. Section rows come first (in MASM's section-table order, supplied
//! by the caller), then program symbols in ASCII byte order (so uppercase sorts
//! before lowercase), then a `N symbols` footer counting sections + program syms.

use crate::encoder::{LineEmit, ListLine};
use crate::symbols::{Kind, SymbolTable};

/// Render the listing **body**: one row per listed source line, in the form
///
/// ```text
/// Abs. Rel.   Loc    Obj. code   Source line
/// 1074   19i  000200 0000        PART_NUMBER_VECTOR_1  FDB  PAGE(PART_NUMBER)
/// ```
///
/// Column layout (0-based): `Abs`[0..4] ` ` `Rel`[5..9] (the include `i` suffix
/// sits at col 9) `  ` `Loc`[12..18] ` ` `Obj. code`[19..28] `   ` source from
/// col 31. Object bytes are grouped into big-endian 16-bit words; a line emitting
/// more than two words wraps onto continuation rows that repeat `Abs`/`Rel`,
/// advance `Loc`, and carry no source. `equ` shows its value as a 32-bit word
/// pair in `Obj. code` with a blank `Loc`. Source tabs expand at 8-column stops.
///
/// `line_emit` is indexed positionally by the assembled stream: its k-th entry is
/// the k-th `assembled` line in `list_lines`. (Page headers/pagination are added
/// by a separate layer; this renders the continuous body.)
pub fn body(list_lines: &[ListLine], line_emit: &[LineEmit]) -> String {
    let mut out = String::new();
    let mut asm_idx = 0usize;
    for (i, ll) in list_lines.iter().enumerate() {
        let emit = if ll.assembled {
            let e = line_emit.get(asm_idx);
            asm_idx += 1;
            e
        } else {
            None
        };
        if !ll.listed {
            continue;
        }
        let abs = i + 1;
        let isuf = if ll.depth > 0 { 'i' } else { ' ' };
        // Prefix occupies cols 0..12 (wider when `Abs`/`Rel` exceed 4 digits), so
        // `Loc` begins at col 12.
        let pre = format!("{abs:>4} {rel:>4}{isuf}  ", rel = ll.rel);
        // Instruction wraps blank `Rel` on continuation rows; the abs field keeps
        // its width and the `Rel`+suffix+gap (8 cols) become spaces.
        let pre_cont = format!("{abs:>4}        ");
        // MASM caps the source column at 132 chars (the rest of a long comment is
        // dropped); detab first since the cap is on displayed columns.
        let src: String = detab(&ll.text).chars().take(SRC_WIDTH).collect();

        match emit {
            Some(e) if !e.bytes.is_empty() => {
                let words = group_words(&e.bytes);
                let loc0 = e.loc.unwrap_or(0);
                let mut byte_off = 0u32;
                // Up to two 16-bit words per physical line; the rest wrap. Data
                // directives repeat Abs+Rel on continuation rows; instructions
                // repeat Abs but blank Rel.
                for (k, chunk) in words.chunks(2).enumerate() {
                    let obj = chunk.join(" ");
                    let (p, s) = if k == 0 {
                        (pre.as_str(), src.as_str())
                    } else if e.is_data {
                        (pre.as_str(), "")
                    } else {
                        (pre_cont.as_str(), "")
                    };
                    body_row(&mut out, p, Some(loc0 + byte_off), &obj, s);
                    byte_off += chunk.iter().map(|w| (w.len() / 2) as u32).sum::<u32>();
                }
            }
            Some(e) if e.equ.is_some() => {
                let v = e.equ.unwrap();
                let obj = format!("{:04X} {:04X}", v >> 16, v & 0xFFFF);
                body_row(&mut out, &pre, None, &obj, &src);
            }
            Some(e) if e.loc.is_some() => body_row(&mut out, &pre, e.loc, "", &src),
            _ => body_row(&mut out, &pre, None, "", &src),
        }
    }
    // Body footer: a blank line then the total source-line count. MASM counts the
    // empty line after the file's final newline (which `lines()` drops), so the
    // tally is one past the highest `Abs`.
    out.push_str(&format!("\r\n{} lines assembled\r\n", list_lines.len() + 1));
    out
}

/// Emit one physical body row. `loc` is the 6-hex `Loc` (blank if `None`); `obj`
/// the object-code text (≤9 chars, left-justified); `src` the source column.
fn body_row(out: &mut String, pre: &str, loc: Option<u32>, obj: &str, src: &str) {
    let loc6 = match loc {
        Some(l) => format!("{l:06X}"),
        None => "      ".to_string(),
    };
    // The `.LST` is a DOS text file: CRLF line endings. (`src` is pre-truncated.)
    out.push_str(&format!("{pre}{loc6} {obj:<9}   {src}\r\n"));
}

/// Maximum width of the source column; longer source comments are cut here.
const SRC_WIDTH: usize = 132;

/// Group bytes into big-endian 16-bit words for the `Obj. code` column: each pair
/// renders as four hex digits, a trailing odd byte as two.
fn group_words(bytes: &[u8]) -> Vec<String> {
    bytes
        .chunks(2)
        .map(|c| {
            if c.len() == 2 {
                format!("{:02X}{:02X}", c[0], c[1])
            } else {
                format!("{:02X}", c[0])
            }
        })
        .collect()
}

/// Expand tabs to 8-column stops, measured from the source field's own column 0
/// (MASM detabs each source line before placing it in the listing).
fn detab(s: &str) -> String {
    if !s.contains('\t') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 8);
    let mut col = 0usize;
    for ch in s.chars() {
        if ch == '\t' {
            let n = 8 - (col % 8);
            out.extend(std::iter::repeat(' ').take(n));
            col += n;
        } else {
            out.push(ch);
            col += 1;
        }
    }
    out
}

/// Render the Symbol Table block (no surrounding page headers). `sections` is the
/// `(name, vaddr)` of each emitted section in MASM symbol-table order; `macros`
/// the names of every defined macro (listed with attrib `macro`, no value).
pub fn symbol_table(symbols: &SymbolTable, macros: &[String], sections: &[(&str, u32)]) -> String {
    let mut out = String::new();
    out.push_str("Symbol Table:\n\n");
    out.push_str("symbol name       attrib.   section    value\n");
    out.push_str("-----------       -------   -------    -----\n");

    for (name, vaddr) in sections {
        let secnum = if *name == ".bss" { "0" } else { "249" };
        out.push_str(&row(name, "section", secnum, *vaddr));
    }

    // Program symbols and macros share one ASCII-sorted list (uppercase before
    // lowercase). Macros carry no value.
    let mac: std::collections::HashSet<&str> = macros.iter().map(String::as_str).collect();
    let mut names: Vec<&str> = symbols.iter().map(|(n, _)| n.as_str()).chain(mac.iter().copied()).collect();
    names.sort_unstable();
    for name in &names {
        if mac.contains(name) {
            out.push_str(&macro_row(name));
        } else {
            let (value, kind) = symbols.get_full(name).unwrap();
            let (attrib, secnum) = match kind {
                Kind::Abs => ("abs", ""),
                Kind::Rel => (".asct", "249"),
            };
            out.push_str(&row(name, attrib, secnum, value as u32));
        }
    }

    out.push_str(&format!("\n{} symbols\n", sections.len() + names.len()));
    out
}

fn row(name: &str, attrib: &str, section: &str, value: u32) -> String {
    format!("{name:<16}  {attrib:<8}  {section:<9}  0x{value:08x}\n")
}

/// A macro's symbol-table row: name, attrib `macro`, no section/value. MASM pads
/// the line to column 40 (one past where a value would begin).
fn macro_row(name: &str) -> String {
    format!("{:<40}\n", format!("{name:<16}  macro"))
}

#[cfg(test)]
mod tests {
    use crate::encoder::{assemble_source, LineEmit, ListLine};

    fn ll(text: &str, rel: u32, depth: u32) -> ListLine {
        ListLine { text: text.to_string(), rel, depth, listed: true, assembled: true }
    }

    // Exercises every body rule: blank Loc/Obj on a comment; instruction object
    // code that wraps with a *blank* Rel continuation; data (fcb) object code that
    // wraps repeating Abs+Rel; the `equ` 32-bit value pair; an `rmb` Loc with no
    // Obj; and the include `i` suffix. Layout verified byte-exact on the full jte
    // listing; this pins the format so regressions surface fast.
    #[test]
    fn body_layout_and_wrapping() {
        let lines = [
            ll("* note", 1, 0),
            ll("  brclr foo", 2, 0),
            ll("  fcb 1,2,3,4,5,6", 3, 0),
            ll("K       equ  $FF", 4, 0),
            ll("R       rmb  4", 5, 0),
            ll("        nop", 9, 1),
        ];
        let emit = [
            LineEmit::default(),
            LineEmit { loc: Some(0x2000), bytes: vec![0x37, 0x2A, 0x80, 0x01, 0x10, 0x0A], ..Default::default() },
            LineEmit { loc: Some(0x2010), bytes: vec![1, 2, 3, 4, 5, 6], is_data: true, ..Default::default() },
            LineEmit { equ: Some(0x00FF), ..Default::default() },
            LineEmit { loc: Some(0x2020), ..Default::default() },
            LineEmit { loc: Some(0x2024), bytes: vec![0x27], ..Default::default() },
        ];
        let got = super::body(&lines, &emit);
        let want = concat!(
            "   1    1                      * note\r\n",
            "   2    2   002000 372A 8001     brclr foo\r\n",
            "   2        002004 100A        \r\n",
            "   3    3   002010 0102 0304     fcb 1,2,3,4,5,6\r\n",
            "   3    3   002014 0506        \r\n",
            "   4    4          0000 00FF   K       equ  $FF\r\n",
            "   5    5   002020             R       rmb  4\r\n",
            "   6    9i  002024 27                  nop\r\n",
            "\r\n7 lines assembled\r\n",
        );
        assert_eq!(got, want);
    }

    #[test]
    fn body_truncates_source_at_132() {
        let long = format!("* {}", "x".repeat(200));
        let lines = [ll(&long, 1, 0)];
        let emit = [LineEmit::default()];
        let got = super::body(&lines, &emit);
        // Prefix (31) + 132-char source + CRLF, then the footer.
        let first = got.lines().next().unwrap();
        assert_eq!(first.len(), 31 + 132);
        assert!(first.ends_with("xxx"));
    }


    // The Symbol Table MASM 4.6 emits for this exact source (captured from the
    // DOSBox oracle), modulo the section rows which the caller orders.
    const PROBE: &str = "        ASCT\n\
                         \x20       ORG  $2000\n\
                         CODE1   nop\n\
                         \x20       ldab #$12\n\
                         \x20       rts\n\
                         WORD1   fdb  $1234,$5678\n\
                         BYTE1   fcb  $56\n\
                         TABLE   equ  *\n\
                         \x20       fcb  $11,$22,$33\n\
                         RES1    rmb  4\n\
                         PASTRES equ  *\n\
                         KONST   equ  $00FF\n\
                         DERIVED equ  KONST+1\n\
                         \x20       end\n";

    #[test]
    fn symbol_table_matches_oracle() {
        let obj = assemble_source(PROBE);
        // Probe has one .asct (org $2000) + the always-present .bss; the symbol
        // table lists code/data sections first, .bss last.
        let sections = [(".asct", 0x2000u32), (".bss", 0u32)];
        let got = super::symbol_table(&obj.symbols, &[], &sections);
        let want = "\
Symbol Table:

symbol name       attrib.   section    value
-----------       -------   -------    -----
.asct             section   249        0x00002000
.bss              section   0          0x00000000
BYTE1             .asct     249        0x0000200a
CODE1             .asct     249        0x00002000
DERIVED           abs                  0x00000100
KONST             abs                  0x000000ff
PASTRES           .asct     249        0x00002012
RES1              .asct     249        0x0000200e
TABLE             .asct     249        0x0000200b
WORD1             .asct     249        0x00002006

10 symbols
";
        assert_eq!(got, want);
    }
}
