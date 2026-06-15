//! M68HC16 (CPU16) assembler library.
//!
//! Public surface is intentionally tiny while the skeleton settles. The
//! [`assemble`] entry point will eventually drive the lexer, parser, encoder,
//! and output writers; for now it is a placeholder so the CLI and GUI binaries
//! can compile and exercise the wiring end-to-end.

use std::path::{Path, PathBuf};

pub mod diag;
pub mod directives;
pub mod encoder;
pub mod expr;
pub mod isa;
pub mod lexer;
pub mod output;
pub mod parser;
pub mod symbols;

use diag::Diagnostic;

/// Inputs and configuration for a single assembler run.
#[derive(Debug, Clone)]
pub struct AssembleRequest {
    pub input: PathBuf,
    pub output_dir: PathBuf,
}

/// Files produced by a successful assembler run.
#[derive(Debug, Default, Clone)]
pub struct AssembleOutputs {
    pub s_record: Option<PathBuf>,
    pub listing: Option<PathBuf>,
    pub map: Option<PathBuf>,
}

/// Result of an assembler run. Diagnostics may be present even on success
/// (warnings); failures will produce at least one `Severity::Error` entry.
#[derive(Debug, Default)]
pub struct AssembleResult {
    pub outputs: AssembleOutputs,
    pub diagnostics: Vec<Diagnostic>,
}

impl AssembleResult {
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.is_error())
    }
}

/// Assemble a single source file: parse, encode, and write the `.S19` image.
/// (Listing/map outputs follow once their writers land.)
pub fn assemble(req: &AssembleRequest) -> AssembleResult {
    let mut result = AssembleResult::default();

    if !req.input.exists() {
        result
            .diagnostics
            .push(Diagnostic::error(format!("input file not found: {}", req.input.display())));
        return result;
    }
    if !req.output_dir.exists() {
        result.diagnostics.push(Diagnostic::error(format!(
            "output directory not found: {}",
            req.output_dir.display()
        )));
        return result;
    }

    let bytes = match std::fs::read(&req.input) {
        Ok(b) => b,
        Err(e) => {
            result
                .diagnostics
                .push(Diagnostic::error(format!("cannot read {}: {e}", req.input.display())));
            return result;
        }
    };
    // DOS sources are Latin-1/codepage text; decode byte-for-byte so the listing
    // reproduces them exactly (assembled tokens are ASCII, so unaffected).
    let src = encoder::decode_latin1(&bytes);

    let obj = encoder::assemble_source_in(&src, req.input.parent());
    result.diagnostics = obj.diagnostics;
    if result.has_errors() {
        return result;
    }

    let stem = req.input.file_stem().and_then(|s| s.to_str()).unwrap_or("out");

    // Relocatable COFF object (the intermediate HEX.exe converts to the S-record).
    // Timestamp is left 0; it is the only non-deterministic field MASM writes.
    let obj_path = req.output_dir.join(format!("{stem}.OBJ"));
    let obj_bytes = output::coff::write_coff(&obj.data, &obj.spans, &obj.symbols, &obj.sym_order, 0);
    if let Err(e) = std::fs::write(&obj_path, obj_bytes) {
        result
            .diagnostics
            .push(Diagnostic::error(format!("cannot write {}: {e}", obj_path.display())));
    }

    // Dev validation hook: dump the listing's Symbol Table block (env HC16_LST).
    if let Ok(path) = std::env::var("HC16_LST") {
        let secs = output::coff::section_list(&obj.data, &obj.spans);
        let _ = std::fs::write(&path, output::listing::symbol_table(&obj.symbols, &obj.macros, &secs));
    }
    // Dev validation hook: dump the listing body (env HC16_LSTBODY).
    if let Ok(path) = std::env::var("HC16_LSTBODY") {
        let body = output::listing::body(&obj.list_lines, &obj.line_emit);
        let _ = std::fs::write(&path, output::encode_latin1(&body));
    }
    // Dev validation hook: dump the paginated body (env HC16_LSTPAGE). Uses the
    // oracle's top-file name and a fixed timestamp; the real per-page timestamp is
    // non-deterministic (wall-clock), so comparisons normalise it.
    if let Ok(path) = std::env::var("HC16_LSTPAGE") {
        let opts = output::listing::PageOpts {
            top_file: "IN.ASM",
            timestamp: "Mon Jun 15 09:39:05 ",
            plen: 60,
        };
        let page = output::listing::paginate_body(&obj.list_lines, &obj.line_emit, &opts);
        let _ = std::fs::write(&path, output::encode_latin1(&page));
    }

    let s19_path = req.output_dir.join(format!("{stem}.S19"));
    // HEX.exe converts `<name>.OBJ`, and records that input name in the S0 header.
    let text = output::srec::write_srecords(&obj.data, &format!("{stem}.OBJ"));
    match std::fs::write(&s19_path, text) {
        Ok(()) => result.outputs.s_record = Some(s19_path),
        Err(e) => result
            .diagnostics
            .push(Diagnostic::error(format!("cannot write {}: {e}", s19_path.display()))),
    }

    result
}

/// Convenience helper used by tests: derive the expected sibling output paths
/// for a given input `.asm` file. The naming matches what MASM.exe produces.
pub fn expected_output_paths(input: &Path) -> AssembleOutputs {
    let stem = input.file_stem().unwrap_or_default();
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let with_ext = |ext: &str| {
        let mut p = parent.join(stem);
        p.set_extension(ext);
        p
    };
    AssembleOutputs {
        s_record: Some(with_ext("S19")),
        listing: Some(with_ext("LST")),
        map: Some(with_ext("MAP")),
    }
}
