//! Keeps the VS Code grammar (`editors/vscode`) in step with the ISA table: every
//! CPU16 mnemonic the assembler encodes must be highlighted as one, and in the
//! right group (no-operand instructions treat the rest of the line as comment).

use m68hc16_asm::isa::{Mode, INSTRUCTIONS};

const GRAMMAR: &str = include_str!("../../../editors/vscode/syntaxes/m68hc16.tmLanguage.json");

/// The alternation inside `(?i:\b(...)\b)` of the repository rule `rule`.
fn words(rule: &str) -> Vec<String> {
    let start = GRAMMAR.find(&format!("\"{rule}\": {{")).unwrap_or_else(|| panic!("rule {rule} missing"));
    let rest = &GRAMMAR[start..];
    let a = rest.find(r"(?i:\\b(").expect("alternation") + r"(?i:\\b(".len();
    let b = a + rest[a..].find(r")\\b)").expect("alternation end");
    rest[a..b].split('|').map(str::to_string).collect()
}

#[test]
fn grammar_mnemonics_match_isa_table() {
    let inherent = words("inherent");
    let operand = words("instruction");
    for insn in INSTRUCTIONS {
        let is_inherent = insn.modes.iter().all(|m| m.mode == Mode::Inherent);
        let (want, other) = if is_inherent { (&inherent, &operand) } else { (&operand, &inherent) };
        assert!(want.iter().any(|w| w == insn.mnemonic), "`{}` missing from grammar", insn.mnemonic);
        assert!(!other.iter().any(|w| w == insn.mnemonic), "`{}` in the wrong grammar group", insn.mnemonic);
    }
    assert_eq!(inherent.len() + operand.len(), INSTRUCTIONS.len(), "grammar lists a mnemonic the ISA lacks");
}
