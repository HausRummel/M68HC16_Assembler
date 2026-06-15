//! Fixture-driven byte-exact comparison harness.
//!
//! Discovers `*.asm` files under `tests/fixtures/public/` (and, when the
//! `private-fixtures` feature is enabled, under the path named in
//! `private_path.toml`), runs the assembler, and diffs the produced
//! `.S19` / `.LST` / `.MAP` against sibling reference files.
//!
//! Currently a skeleton: if no fixtures are present, the test silently passes.

use std::fs;
use std::path::{Path, PathBuf};

fn fixture_roots() -> Vec<PathBuf> {
    // CARGO_MANIFEST_DIR is the library crate's directory at test-build time.
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    #[allow(unused_mut)]
    let mut roots = vec![crate_dir.join("tests/fixtures/public")];

    #[cfg(feature = "private-fixtures")]
    {
        let workspace_root = crate_dir
            .join("../..")
            .canonicalize()
            .unwrap_or_else(|_| crate_dir.clone());
        let cfg = workspace_root.join("private_path.toml");
        if let Ok(text) = fs::read_to_string(&cfg) {
            for line in text.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("path") {
                    let rest = rest.trim_start_matches([' ', '=']).trim();
                    let unquoted = rest.trim_matches('"').trim_matches('\'');
                    if !unquoted.is_empty() {
                        roots.push(PathBuf::from(unquoted));
                    }
                }
            }
        }
    }

    roots
}

fn discover_asm(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(discover_asm(&path));
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("asm"))
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
    out
}

#[test]
fn golden_fixtures() {
    let (mut total, mut checked) = (0usize, 0usize);
    for root in fixture_roots() {
        for asm in discover_asm(&root) {
            total += 1;
            // A `.bytes` sibling (space-separated hex of the MASM image) is the
            // oracle ground truth; assemble and compare byte-for-byte.
            let expected_path = asm.with_extension("bytes");
            let Ok(expected_text) = fs::read_to_string(&expected_path) else {
                continue;
            };
            let src = fs::read_to_string(&asm).expect("read .asm");
            let obj = m68hc16_asm::encoder::assemble_source(&src);
            assert!(
                !obj.has_errors(),
                "{}: diagnostics: {:?}",
                asm.display(),
                obj.diagnostics
            );
            let expected = parse_hex(&expected_text);
            let actual = obj.bytes();
            assert_eq!(
                actual,
                expected,
                "{}: byte mismatch\n  actual:   {}\n  expected: {}",
                asm.display(),
                hex(&actual),
                hex(&expected)
            );
            checked += 1;
        }
    }
    if total == 0 {
        eprintln!("golden_fixtures: no fixtures discovered (skipping)");
    } else {
        eprintln!("golden_fixtures: {checked}/{total} fixtures byte-checked");
    }
}

fn parse_hex(s: &str) -> Vec<u8> {
    s.split_whitespace()
        .map(|t| u8::from_str_radix(t, 16).expect("hex byte"))
        .collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
}
