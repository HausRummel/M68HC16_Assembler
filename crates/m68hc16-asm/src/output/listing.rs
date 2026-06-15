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

use crate::symbols::{Kind, SymbolTable};

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
    use crate::encoder::assemble_source;

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
