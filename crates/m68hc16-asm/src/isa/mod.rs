//! M68HC16 (CPU16) instruction-set tables.
//!
//! The encoding data in [`INSTRUCTIONS`] is generated into `table.rs` by
//! `tools/oracle/Generate-IsaRust.ps1` from `docs/spec/isa-table.tsv`, which is
//! itself derived from the real Motorola MASM via the DOSBox golden oracle — so
//! every prefix here is authoritative CPU16 machine code, not a guess.
//!
//! Each [`ModeEntry`] gives the opcode/prebyte bytes (`prefix`) and how many
//! `operand` bytes follow. The encoder selects a mode from the parsed operand,
//! emits the prefix, then appends the operand bytes per that mode's layout.

mod table;

pub use table::INSTRUCTIONS;

/// Index register used by indexed addressing modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdxReg {
    X,
    Y,
    Z,
}

/// CPU16 addressing modes the assembler encodes.
///
/// The `operand_len` on each [`ModeEntry`] records how many bytes the operand
/// contributes; the sub-layout (immediate vs offset vs mask+addr) is implied by
/// the mode and handled in the encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// No operand; the prefix is the whole instruction.
    Inherent,
    /// 8-bit immediate.
    Imm8,
    /// 16-bit immediate.
    Imm16,
    /// 16-bit extended address.
    Ext,
    /// 20-bit extended address (e.g. `jmp`/`jsr`).
    Ext20,
    /// 8-bit unsigned offset, indexed by X/Y/Z.
    Ind8(IdxReg),
    /// 16-bit offset, indexed by X/Y/Z.
    Ind16(IdxReg),
    /// 20-bit offset (3 bytes), indexed by X/Y/Z — used by `jmp`/`jsr` through an
    /// index register to reach any code page. Always 3 bytes, never span-dependent.
    Ind20(IdxReg),
    /// Accumulator-E offset, indexed by X/Y/Z (no operand bytes).
    EInd(IdxReg),
    /// 8-bit PC-relative branch.
    Rel8,
    /// 16-bit PC-relative (long) branch.
    Rel16,
    /// Bit op on an extended address: `mask8`, `addr16`.
    BitExt,
    /// Bit op on an 8-bit indexed address: `mask8`, `off8`.
    BitInd(IdxReg),
    /// Bit op on a 16-bit indexed address: `mask8`, `off16`.
    BitInd16(IdxReg),
    /// WORD bit op (`bsetw`/`bclrw`) on a 16-bit indexed address. Unlike the byte
    /// forms this emits `off16`, `mask16` (offset first, then a 16-bit mask) and has
    /// no 8-bit-offset variant. Prefix `0x27` (vs `0x17` for the byte form).
    BitIndW(IdxReg),
    /// Bit-conditional branch (extended): `mask8`, `addr16`, `rel16`.
    BitBrExt,
    /// Bit-conditional branch (8-bit indexed): `mask8`, `off8`, `rel8`.
    BitBrInd(IdxReg),
    /// Bit-conditional branch (16-bit indexed): `mask8`, `off16`, `rel16`.
    BitBrInd16(IdxReg),
    /// `pshm`/`pulm` register-mask byte.
    RegList,
    /// `movb`/`movw` memory-to-memory: two 16-bit addresses.
    MovMm,
    /// `movb`/`movw` indexed source to extended destination.
    MovIdxExt,
    /// `movb`/`movw` extended source to indexed destination.
    MovExtIdx,
    /// `rmac` packed signed offsets.
    Mac,
}

/// One legal `(mode, encoding)` for a mnemonic.
#[derive(Debug, Clone, Copy)]
pub struct ModeEntry {
    pub mode: Mode,
    /// Opcode/prebyte bytes emitted before the operand.
    pub prefix: &'static [u8],
    /// Number of operand bytes that follow the prefix.
    pub operand_len: u8,
}

/// A mnemonic and all of its legal addressing modes.
#[derive(Debug, Clone, Copy)]
pub struct InsnDef {
    pub mnemonic: &'static str,
    pub modes: &'static [ModeEntry],
}

impl InsnDef {
    /// The entry for a specific mode, if this mnemonic supports it.
    pub fn mode(&self, mode: Mode) -> Option<&'static ModeEntry> {
        self.modes.iter().find(|m| m.mode == mode)
    }
}

/// Look up a mnemonic (case-insensitive — MASM source is case-insensitive).
pub fn lookup(mnemonic: &str) -> Option<&'static InsnDef> {
    INSTRUCTIONS
        .iter()
        .find(|d| d.mnemonic.eq_ignore_ascii_case(mnemonic))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix_of(mnemonic: &str, mode: Mode) -> &'static [u8] {
        lookup(mnemonic)
            .unwrap_or_else(|| panic!("mnemonic {mnemonic} not found"))
            .mode(mode)
            .unwrap_or_else(|| panic!("{mnemonic} has no mode {mode:?}"))
            .prefix
    }

    #[test]
    fn table_is_nonempty_and_sorted_lookup_works() {
        assert!(INSTRUCTIONS.len() > 200, "expected the full corpus ISA");
        assert!(lookup("LDAA").is_some(), "lookup is case-insensitive");
        assert!(lookup("not_an_insn").is_none());
    }

    #[test]
    fn known_encodings_match_masm() {
        // Authoritative bytes captured from the real MASM via the oracle.
        assert_eq!(prefix_of("ldaa", Mode::Imm8), &[0x75]);
        assert_eq!(prefix_of("ldaa", Mode::Ext), &[0x17, 0x75]);
        assert_eq!(prefix_of("ldaa", Mode::Ind8(IdxReg::X)), &[0x45]);
        assert_eq!(prefix_of("ldaa", Mode::Ind8(IdxReg::Y)), &[0x55]);
        assert_eq!(prefix_of("ldaa", Mode::Ind8(IdxReg::Z)), &[0x65]);
        assert_eq!(prefix_of("ldd", Mode::Imm16), &[0x37, 0xB5]);
        assert_eq!(prefix_of("std", Mode::Ext), &[0x37, 0xFA]);
        assert_eq!(prefix_of("rts", Mode::Inherent), &[0x27, 0xF7]);
        assert_eq!(prefix_of("bra", Mode::Rel8), &[0xB0]);
        assert_eq!(prefix_of("lbra", Mode::Rel16), &[0x37, 0x80]);
        assert_eq!(prefix_of("jmp", Mode::Ext20), &[0x7A]);
        assert_eq!(prefix_of("bset", Mode::BitExt), &[0x39]);
        assert_eq!(prefix_of("brset", Mode::BitBrExt), &[0x3B]);
        assert_eq!(prefix_of("pshm", Mode::RegList), &[0x34]);
    }

    #[test]
    fn post_corpus_mnemonics_present_and_correct() {
        // Instructions absent from our original corpus but valid HC16 — added
        // after a different source release used them (TSX/PSHX/NEG and siblings).
        // All bytes captured from the real MASM via the oracle.
        for m in ["tsx", "tsz", "txs", "tys", "tzs", "pshx", "pulx", "neg", "negw"] {
            assert!(lookup(m).is_some(), "missing mnemonic `{m}`");
        }
        // Transfers: TSX/TSZ live on page 0x27, but TXS/TYS/TZS live on 0x37 —
        // a structural asymmetry that must come from the oracle, not inference.
        assert_eq!(prefix_of("tsx", Mode::Inherent), &[0x27, 0x4F]);
        assert_eq!(prefix_of("tsz", Mode::Inherent), &[0x27, 0x6F]);
        assert_eq!(prefix_of("txs", Mode::Inherent), &[0x37, 0x4E]);
        assert_eq!(prefix_of("tys", Mode::Inherent), &[0x37, 0x5E]);
        assert_eq!(prefix_of("tzs", Mode::Inherent), &[0x37, 0x6E]);
        // PSHX/PULX expand to PSHM/PULM masks — and the masks differ (0x04 vs 0x10)
        // because PSHM/PULM use mirrored bit orders so a push/pull pair round-trips.
        assert_eq!(prefix_of("pshx", Mode::Inherent), &[0x34, 0x04]);
        assert_eq!(prefix_of("pulx", Mode::Inherent), &[0x35, 0x10]);
        // NEG memory read-modify-write: byte form (0x17/bare) and word form (0x27).
        assert_eq!(prefix_of("neg", Mode::Ext), &[0x17, 0x32]);
        assert_eq!(prefix_of("neg", Mode::Ind8(IdxReg::X)), &[0x02]);
        assert_eq!(prefix_of("neg", Mode::Ind16(IdxReg::Z)), &[0x17, 0x22]);
        assert_eq!(prefix_of("negw", Mode::Ext), &[0x27, 0x32]);
        // mac (single multiply-accumulate) shares rmac's packed Mac mode.
        assert_eq!(prefix_of("mac", Mode::Mac), &[0x7B]);
    }

    #[test]
    fn operand_len_consistent_with_mode() {
        for d in INSTRUCTIONS {
            for m in d.modes {
                match m.mode {
                    Mode::Inherent | Mode::EInd(_) => assert_eq!(m.operand_len, 0, "{} {:?}", d.mnemonic, m.mode),
                    Mode::Imm8 | Mode::Rel8 => assert_eq!(m.operand_len, 1, "{} {:?}", d.mnemonic, m.mode),
                    Mode::Imm16 | Mode::Ext | Mode::Rel16 => assert_eq!(m.operand_len, 2, "{} {:?}", d.mnemonic, m.mode),
                    Mode::Ext20 | Mode::Ind20(_) => assert_eq!(m.operand_len, 3, "{} {:?}", d.mnemonic, m.mode),
                    _ => assert!(m.operand_len <= 5, "{} {:?}", d.mnemonic, m.mode),
                }
            }
        }
    }
}
