//! Editor features computed from an [`Analysis`]: words under the cursor,
//! hover text, and diagnostic ranges. Kept free of LSP plumbing so it can be
//! tested directly.

use m68hc16_asm::diag::Severity;
use m68hc16_asm::isa::{self, IdxReg, Mode};
use m68hc16_asm::{SymbolInfo, SymbolKind};

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '?'
}

/// The identifier touching character column `col` (0-based) of `line`, with
/// its start column. Columns count `char`s, which equal UTF-16 units for the
/// Latin-1 / CP437 text these sources hold.
pub fn word_at(line: &str, col: usize) -> Option<(String, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut start = col.min(chars.len());
    // Allow the cursor just past the end of a word.
    if (start == chars.len() || !is_word_char(chars[start])) && start > 0 && is_word_char(chars[start - 1]) {
        start -= 1;
    }
    if start >= chars.len() || !is_word_char(chars[start]) {
        return None;
    }
    while start > 0 && is_word_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = start;
    while end < chars.len() && is_word_char(chars[end]) {
        end += 1;
    }
    let w: String = chars[start..end].iter().collect();
    // A leading digit is a number, not a symbol.
    if w.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some((w, start))
}

/// Length in chars of the token starting at `col` (0-based), for a squiggle.
/// Quoted operands run to the closing quote; anything else to the next blank
/// or comma. At least 1.
pub fn token_len(line: &str, col: usize) -> usize {
    let chars: Vec<char> = line.chars().skip(col).collect();
    let n = match chars.first() {
        Some(&q @ ('"' | '\'')) => chars[1..].iter().position(|&c| c == q).map(|p| p + 2).unwrap_or(chars.len()),
        Some(_) => chars.iter().position(|&c| c.is_whitespace() || c == ',').unwrap_or(chars.len()),
        None => 0,
    };
    n.max(1)
}

pub fn fmt_hex(v: i64) -> String {
    if v < 0 {
        format!("{v}")
    } else if v > 0xFFFF {
        format!("${v:05X}")
    } else if v > 0xFF {
        format!("${v:04X}")
    } else {
        format!("${v:02X}")
    }
}

/// Markdown hover for a symbol.
pub fn symbol_hover(s: &SymbolInfo, def_display: Option<String>) -> String {
    let mut out = format!("**{}**", s.name);
    match (s.kind, s.value) {
        (SymbolKind::Macro, _) => out.push_str(" (macro)"),
        (SymbolKind::Constant, Some(v)) => {
            out.push_str(&format!(" (constant)\n\n`{}` = {v}", fmt_hex(v)));
        }
        (SymbolKind::Address, Some(v)) => {
            out.push_str(&format!(" (address)\n\n`{}`", fmt_hex(v)));
            if (0..=0xF_FFFF).contains(&v) {
                out.push_str(&format!(", bank (PK) `${:X}`, offset `${:04X}`", v >> 16, v & 0xFFFF));
            }
        }
        _ => {}
    }
    if let Some(d) = def_display {
        out.push_str(&format!("\n\nDefined at {d}"));
    }
    out
}

fn reg(r: IdxReg) -> &'static str {
    match r {
        IdxReg::X => "X",
        IdxReg::Y => "Y",
        IdxReg::Z => "Z",
    }
}

/// Operand syntax for an addressing mode, as it is written in source.
pub fn mode_syntax(m: Mode) -> String {
    match m {
        Mode::Inherent => "(no operand)".into(),
        Mode::Imm8 => "#imm8".into(),
        Mode::Imm16 => "#imm16".into(),
        Mode::Ext => "addr16".into(),
        Mode::Ext20 => "addr20".into(),
        Mode::Ind8(r) => format!("off8,{}", reg(r)),
        Mode::Ind16(r) => format!("off16,{}", reg(r)),
        Mode::Ind20(r) => format!("off20,{}", reg(r)),
        Mode::EInd(r) => format!("E,{}", reg(r)),
        Mode::Rel8 => "rel8".into(),
        Mode::Rel16 => "rel16".into(),
        Mode::BitExt => "addr16,#mask".into(),
        Mode::BitInd(r) => format!("off8,{},#mask", reg(r)),
        Mode::BitInd16(r) => format!("off16,{},#mask", reg(r)),
        Mode::BitIndW(r) => format!("off16,{},#mask16", reg(r)),
        Mode::BitBrExt => "addr16,#mask,target".into(),
        Mode::BitBrInd(r) => format!("off8,{},#mask,target", reg(r)),
        Mode::BitBrInd16(r) => format!("off16,{},#mask,target", reg(r)),
        Mode::RegList => "reglist".into(),
        Mode::MovMm => "addr16,addr16".into(),
        Mode::MovIdxExt => "off,X,addr16".into(),
        Mode::MovExtIdx => "addr16,off,X".into(),
        Mode::Mac => "xo,yo".into(),
    }
}

/// Markdown hover for a CPU16 mnemonic: each addressing mode and its opcode.
pub fn mnemonic_hover(word: &str) -> Option<String> {
    let insn = isa::lookup(word)?;
    let mut out = format!("**{}** (CPU16 instruction)\n\n| Operand | Opcode | Operand bytes |\n|---|---|---|\n", insn.mnemonic.to_uppercase());
    for m in insn.modes {
        let op: Vec<String> = m.prefix.iter().map(|b| format!("{b:02X}")).collect();
        out.push_str(&format!("| `{}` | `{}` | {} |\n", mode_syntax(m.mode), op.join(" "), m.operand_len));
    }
    Some(out)
}

/// LSP severity number: 1 error, 2 warning, 3 information.
pub fn severity_num(s: Severity) -> u8 {
    match s {
        Severity::Error => 1,
        Severity::Warning => 2,
        Severity::Note => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_under_cursor() {
        let l = "        LDAA    O2_STATUS-RB,Z";
        assert_eq!(word_at(l, 10), Some(("LDAA".into(), 8)));
        assert_eq!(word_at(l, 16), Some(("O2_STATUS".into(), 16)));
        assert_eq!(word_at(l, 25), Some(("O2_STATUS".into(), 16))); // just past the end
        assert_eq!(word_at(l, 27), Some(("RB".into(), 26)));
        assert_eq!(word_at(l, 2), None);
        assert_eq!(word_at("  ldab #10", 8), None);
    }

    #[test]
    fn token_lengths() {
        assert_eq!(token_len("  ldaa  GONE,X", 8), 4);
        assert_eq!(token_len("  fcc  \"A B\" x", 7), 5);
        assert_eq!(token_len("", 0), 1);
    }

    #[test]
    fn hovers() {
        let s = SymbolInfo { name: "HTR".into(), value: Some(0x2_1234), kind: SymbolKind::Address, def: None };
        let h = symbol_hover(&s, Some("o2/X.ASM:12".into()));
        assert!(h.contains("`$21234`, bank (PK) `$2`, offset `$1234`"), "{h}");
        assert!(h.ends_with("Defined at o2/X.ASM:12"));
        let m = mnemonic_hover("ldaa").unwrap();
        assert!(m.contains("| `off8,X` |"), "{m}");
        assert!(mnemonic_hover("notanop").is_none());
    }
}
