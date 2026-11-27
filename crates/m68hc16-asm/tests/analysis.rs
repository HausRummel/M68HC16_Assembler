//! `analyze`: in-memory sources, symbol definitions, and recovery from
//! preprocessing errors.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use m68hc16_asm::{analyze, SourceLoader, SymbolKind};

/// Sources held in memory, as an editor would serve unsaved buffers.
struct Mem(HashMap<PathBuf, String>);

impl Mem {
    fn new(files: &[(&str, &str)]) -> Self {
        Mem(files.iter().map(|(p, t)| (PathBuf::from(p), t.to_string())).collect())
    }
}

impl SourceLoader for Mem {
    fn load(&self, path: &Path) -> std::io::Result<String> {
        self.0.get(path).cloned().ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
    }
}

const ROOT: &str = "/w/MAIN.ASM";

#[test]
fn symbols_carry_value_kind_and_definition() {
    let mem = Mem::new(&[
        (ROOT, "RB      equ     $10\n        org     $1000\n        include sub/LIB.ASM\nSTART:  nop\n"),
        ("/w/sub/LIB.ASM", "* lib\nDELAY:  macro\n        nop\n        endm\nHELPER: rts\n"),
    ]);
    let a = analyze(Path::new(ROOT), &mem);
    assert!(a.diagnostics.iter().all(|d| !d.is_error()), "{:?}", a.diagnostics);
    assert_eq!(a.files, vec![PathBuf::from(ROOT), PathBuf::from("/w/sub/LIB.ASM")]);

    let rb = a.symbol("RB").unwrap();
    assert_eq!((rb.value, rb.kind), (Some(0x10), SymbolKind::Constant));
    let def = rb.def.as_ref().unwrap();
    assert_eq!((def.file.as_deref(), def.line, def.col), (Some(Path::new(ROOT)), 1, 1));

    let h = a.symbol("HELPER").unwrap();
    assert_eq!((h.value, h.kind), (Some(0x1000), SymbolKind::Address));
    let def = h.def.as_ref().unwrap();
    assert_eq!((def.file.as_deref(), def.line), (Some(Path::new("/w/sub/LIB.ASM")), 5));

    let m = a.symbol("DELAY").unwrap();
    assert_eq!((m.kind, m.def.as_ref().unwrap().line), (SymbolKind::Macro, 2));
}

#[test]
fn unsaved_text_is_used() {
    // The loader is the only source of truth: no file exists on disk.
    let mem = Mem::new(&[(ROOT, "        org     $1000\n        ldaa    NOWHERE\n")]);
    let a = analyze(Path::new(ROOT), &mem);
    let e = a.diagnostics.iter().find(|d| d.is_error()).unwrap();
    assert_eq!(e.loc.as_ref().unwrap().line, 2);
}

#[test]
fn missing_include_does_not_stop_the_run() {
    let mem = Mem::new(&[(
        ROOT,
        "        org     $1000\n        include GONE.ASM\n        ldaa    FROM_GONE\n        bogus\n",
    )]);
    let a = analyze(Path::new(ROOT), &mem);
    let errs: Vec<_> = a.diagnostics.iter().filter(|d| d.is_error()).collect();
    // The include error and the unrelated bad op; the undefined symbol it
    // caused is hidden and counted.
    assert_eq!(errs.len(), 2, "{:?}", a.diagnostics);
    assert!(errs[0].message.starts_with("cannot include"));
    assert_eq!(errs[0].loc.as_ref().unwrap().line, 2);
    assert!(errs[1].message.contains("bogus"));
    assert_eq!(errs[1].loc.as_ref().unwrap().line, 4);
    assert!(a.diagnostics.iter().any(|d| d.message.starts_with("1 undefined-symbol error(s) hidden")));
}

#[test]
fn unclosed_macro_is_located_and_closed() {
    let mem = Mem::new(&[(ROOT, "        org     $1000\nM1:     macro\n        nop\n")]);
    let a = analyze(Path::new(ROOT), &mem);
    let e = a.diagnostics.iter().find(|d| d.is_error()).unwrap();
    assert!(e.message.starts_with("macro not closed"), "{}", e.message);
    let loc = e.loc.as_ref().unwrap();
    assert_eq!((loc.line, loc.col), (2, 9));
    assert_eq!(a.symbol("M1").unwrap().kind, SymbolKind::Macro);
}

#[test]
fn references_cover_definitions_operands_and_macro_calls() {
    let mem = Mem::new(&[
        (ROOT, "RB      equ     $10\n        org     $1000\n        include sub/LIB.ASM\nSTART:  ldaa    RB+1,Z  ; RB in comment\n        DELAY   RB\n"),
        ("/w/sub/LIB.ASM", "DELAY:  macro\n        ldab    #\\1\n        endm\nHELPER: ldx     #RB*2\n        fcc     \"RB\"\n        ldd     $RB\n"),
    ]);
    let a = analyze(Path::new(ROOT), &mem);
    let at = |n: &str| -> Vec<(String, u32, u32)> {
        a.references[n]
            .iter()
            .map(|l| (l.file.as_ref().unwrap().display().to_string(), l.line, l.col))
            .collect()
    };
    assert_eq!(
        at("RB"),
        vec![
            (ROOT.to_string(), 1, 1),
            (ROOT.to_string(), 4, 17),
            (ROOT.to_string(), 5, 17),
            ("/w/sub/LIB.ASM".to_string(), 4, 18),
        ]
    );
    // In file order: the root, then what it includes.
    assert_eq!(at("DELAY"), vec![(ROOT.to_string(), 5, 9), ("/w/sub/LIB.ASM".to_string(), 1, 1)]);
}
