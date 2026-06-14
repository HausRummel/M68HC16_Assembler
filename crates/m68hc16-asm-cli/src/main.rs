use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use m68hc16_asm::{assemble, AssembleRequest};

/// M68HC16 assembler, output-compatible with Motorola MASM.exe.
#[derive(Debug, Parser)]
#[command(name = "m68hc16-asm", version, about, long_about = None)]
struct Cli {
    /// Source file to assemble (`.asm`).
    input: PathBuf,

    /// Directory to place `.S19`, `.LST`, and `.MAP` outputs in.
    /// Defaults to the input file's directory.
    #[arg(short = 'o', long = "output-dir")]
    output_dir: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let output_dir = cli
        .output_dir
        .or_else(|| cli.input.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    let req = AssembleRequest { input: cli.input, output_dir };
    let result = assemble(&req);

    for diag in &result.diagnostics {
        eprintln!("{diag}");
    }

    if result.has_errors() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
