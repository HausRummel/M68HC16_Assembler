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

/// The per-page timestamp MASM writes into `.LST` headers is wall-clock at
/// generation time, so it is non-deterministic. We stamp one fixed value; like
/// the OBJ timestamp it is the only `.LST` field that cannot match a given run.
const LST_TIMESTAMP: &str = "Mon Jun 15 09:39:05 ";

/// Which output files a run should write.
#[derive(Debug, Clone, Copy)]
pub struct Outputs {
    pub obj: bool,
    pub s19: bool,
    pub lst: bool,
    pub bin: bool,
}

impl Default for Outputs {
    /// Matches the original toolchain: `.OBJ`/`.S19`/`.LST` on, raw `.bin` opt-in.
    fn default() -> Self {
        Outputs { obj: true, s19: true, lst: true, bin: false }
    }
}

/// Raw `.bin` image window: a `size`-byte image starting at `base`, gaps and pad
/// set to `fill`. The default is a 256 KB ROM window — base 0, 0x40000 (256 KB),
/// 0xFF — but all three are configurable for other HC16 targets.
#[derive(Debug, Clone, Copy)]
pub struct BinOptions {
    pub fill: u8,
    pub base: u32,
    pub size: u32,
}

impl Default for BinOptions {
    fn default() -> Self {
        BinOptions { fill: 0xFF, base: 0, size: 0x40000 }
    }
}

/// Inputs and configuration for a single assembler run.
#[derive(Debug, Clone)]
pub struct AssembleRequest {
    pub input: PathBuf,
    pub output_dir: PathBuf,
    /// Which files to write.
    pub outputs: Outputs,
    /// `.bin` window/fill (used only when `outputs.bin`).
    pub bin: BinOptions,
}

/// Files produced by a successful assembler run.
#[derive(Debug, Default, Clone)]
pub struct AssembleOutputs {
    pub object: Option<PathBuf>,
    pub s_record: Option<PathBuf>,
    pub binary: Option<PathBuf>,
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
    if req.outputs.obj {
        let obj_path = req.output_dir.join(format!("{stem}.OBJ"));
        let obj_bytes = output::coff::write_coff(&obj.data, &obj.spans, &obj.symbols, &obj.sym_order, obj.asct, 0);
        match std::fs::write(&obj_path, obj_bytes) {
            Ok(()) => result.outputs.object = Some(obj_path),
            Err(e) => result
                .diagnostics
                .push(Diagnostic::error(format!("cannot write {}: {e}", obj_path.display()))),
        }
    }

    // Paginated assembly listing (`.LST`): body + Symbol Table under MASM's page
    // headers. The header filename is the input name and the per-page timestamp is
    // wall-clock in MASM (non-deterministic) — we stamp one fixed value.
    let top_file = req.input.file_name().and_then(|s| s.to_str()).unwrap_or("IN.ASM");
    let secs = output::coff::section_list(&obj.data, &obj.spans, obj.asct);
    if req.outputs.lst {
        let lst_path = req.output_dir.join(format!("{stem}.LST"));
        let opts = output::listing::PageOpts {
            top_file,
            timestamp: LST_TIMESTAMP,
            plen: output::listing::page_length(&obj.list_lines),
        };
        let lst = output::listing::listing(&obj.list_lines, &obj.line_emit, &obj.symbols, &obj.macros, &secs, obj.asct, &opts);
        match std::fs::write(&lst_path, output::encode_latin1(&lst)) {
            Ok(()) => result.outputs.listing = Some(lst_path),
            Err(e) => result
                .diagnostics
                .push(Diagnostic::error(format!("cannot write {}: {e}", lst_path.display()))),
        }
    }

    // Dev validation hook: dump the listing's Symbol Table block (env HC16_LST).
    if let Ok(path) = std::env::var("HC16_LST") {
        let _ = std::fs::write(&path, output::listing::symbol_table(&obj.symbols, &obj.macros, &secs, obj.asct));
    }
    // Dev validation hook: dump the listing body (env HC16_LSTBODY).
    if let Ok(path) = std::env::var("HC16_LSTBODY") {
        let body = output::listing::body(&obj.list_lines, &obj.line_emit);
        let _ = std::fs::write(&path, output::encode_latin1(&body));
    }
    // Dev validation hooks: dump the paginated body / full listing with the oracle's
    // top-file name + a fixed timestamp (the real per-page timestamp is wall-clock,
    // so comparisons normalise it).
    if let Ok(path) = std::env::var("HC16_LSTPAGE").or_else(|_| std::env::var("HC16_LSTFULL")) {
        let oracle_opts = output::listing::PageOpts {
            top_file: "IN.ASM",
            timestamp: LST_TIMESTAMP,
            plen: output::listing::page_length(&obj.list_lines),
        };
        let text = if std::env::var("HC16_LSTFULL").is_ok() {
            output::listing::listing(&obj.list_lines, &obj.line_emit, &obj.symbols, &obj.macros, &secs, obj.asct, &oracle_opts)
        } else {
            output::listing::paginate_body(&obj.list_lines, &obj.line_emit, &oracle_opts).0
        };
        let _ = std::fs::write(&path, output::encode_latin1(&text));
    }

    if req.outputs.s19 {
        let s19_path = req.output_dir.join(format!("{stem}.S19"));
        // HEX.exe converts `<name>.OBJ`, and records that input name in the S0 header.
        let text = output::srec::write_srecords(&obj.data, &format!("{stem}.OBJ"));
        match std::fs::write(&s19_path, text) {
            Ok(()) => result.outputs.s_record = Some(s19_path),
            Err(e) => result
                .diagnostics
                .push(Diagnostic::error(format!("cannot write {}: {e}", s19_path.display()))),
        }
    }

    // Raw binary image (`<stem>.bin`): a fixed `[base, base+size)` window filled with
    // `fill`, the emitted bytes overlaid. Defaults to a 256 KB / 0xFF window.
    if req.outputs.bin {
        let bin_path = req.output_dir.join(format!("{stem}.bin"));
        let (img, dropped) = output::bin::write_binary(&obj.data, req.bin.base, req.bin.size, req.bin.fill);
        match std::fs::write(&bin_path, &img) {
            Ok(()) => {
                result.diagnostics.push(Diagnostic::note(format!(
                    "binary: {} bytes, 0x{:06X}-0x{:06X}, fill 0x{:02X}",
                    img.len(),
                    req.bin.base,
                    req.bin.base.wrapping_add(req.bin.size),
                    req.bin.fill
                )));
                if dropped > 0 {
                    result.diagnostics.push(Diagnostic::warning(format!(
                        "{dropped} byte(s) fell outside the .bin window and were dropped — increase the size/base"
                    )));
                }
                result.outputs.binary = Some(bin_path);
            }
            Err(e) => result
                .diagnostics
                .push(Diagnostic::error(format!("cannot write {}: {e}", bin_path.display()))),
        }
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
        object: Some(with_ext("OBJ")),
        s_record: Some(with_ext("S19")),
        binary: Some(with_ext("bin")),
        listing: Some(with_ext("LST")),
        map: Some(with_ext("MAP")),
    }
}
