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

/// Assemble a single source file. Skeleton implementation: validates inputs
/// and returns an empty result so the binaries can wire up successfully.
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
