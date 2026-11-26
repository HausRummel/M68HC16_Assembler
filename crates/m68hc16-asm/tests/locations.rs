//! Diagnostics point at the original file, line and column, not at the
//! include- and macro-expanded stream the assembler consumes.

use std::path::{Path, PathBuf};

use m68hc16_asm::diag::SourceLoc;
use m68hc16_asm::encoder::assemble_source_in;
use m68hc16_asm::{assemble, AssembleRequest, BinOptions, Outputs};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hc16-loc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    dir
}

fn errors_at(src: &str, dir: &Path) -> Vec<(SourceLoc, String)> {
    assemble_source_in(src, Some(dir))
        .diagnostics
        .into_iter()
        .filter(|d| d.is_error())
        .map(|d| (d.loc.expect("error has a location"), d.message))
        .collect()
}

#[test]
fn error_in_included_file_points_at_that_file() {
    let dir = scratch("inc");
    std::fs::write(dir.join("sub/PATCH.ASM"), "* patch\n        nop\n        ldaa    MISSING\n").unwrap();
    let src = "        org     $1000\n        nop\n        include sub/PATCH.ASM\n        nop\n        bogus\n";
    let errs = errors_at(src, &dir);
    assert_eq!(errs.len(), 2, "{errs:?}");

    let (loc, msg) = &errs[0];
    assert!(msg.contains("MISSING"), "{msg}");
    assert_eq!(loc.file.as_deref(), Some(dir.join("sub/PATCH.ASM").as_path()));
    assert_eq!((loc.line, loc.col), (3, 17));

    // The line after the include is line 5 of the root, not line 7 of the stream.
    let (loc, msg) = &errs[1];
    assert!(msg.contains("bogus"), "{msg}");
    assert_eq!(loc.file, None);
    assert_eq!((loc.line, loc.col), (5, 9));
}

#[test]
fn error_in_macro_expansion_points_at_the_invocation() {
    let dir = scratch("mac");
    let src = "M1:     macro\n        ldaa    \\1\n        endm\n        org     $1000\n        M1      NOPE\n";
    let errs = errors_at(src, &dir);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!((errs[0].0.line, errs[0].0.file.clone()), (5, None));
}

#[test]
fn missing_include_is_located() {
    let dir = scratch("miss");
    let src = "        nop\n        include sub/NOPE.ASM\n";
    let errs = errors_at(src, &dir);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!((errs[0].0.line, errs[0].0.col), (2, 17));
}

#[test]
fn cli_format_names_the_file() {
    let dir = scratch("fmt");
    let input = dir.join("ROOT.ASM");
    std::fs::write(&input, "        org     $1000\n        ldaa    GONE\n").unwrap();
    let req = AssembleRequest {
        input: input.clone(),
        output_dir: dir.clone(),
        outputs: Outputs { obj: false, s19: false, lst: false, bin: false },
        bin: BinOptions::default(),
    };
    let res = assemble(&req);
    let line = res.diagnostics.iter().find(|d| d.is_error()).unwrap().to_string();
    assert_eq!(line, format!("{}:2:17: error: undefined symbol \"GONE\"", input.display()));
}
