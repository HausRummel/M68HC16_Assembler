use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use m68hc16_asm::{assemble, AssembleRequest, BinOptions, Outputs};

/// M68HC16 assembler, output-compatible with Motorola MASM.exe.
#[derive(Debug, Parser)]
#[command(name = "m68hc16-asm", version, about, long_about = None)]
struct Cli {
    /// Source file to assemble (`.asm`).
    input: PathBuf,

    /// Directory to place outputs in. Defaults to the input file's directory.
    #[arg(short = 'o', long = "output-dir")]
    output_dir: Option<PathBuf>,

    /// Also write the raw binary image (`<stem>.bin`).
    #[arg(long = "bin")]
    bin: bool,

    /// Skip the `.OBJ` output.
    #[arg(long = "no-obj")]
    no_obj: bool,
    /// Skip the `.S19` output.
    #[arg(long = "no-s19")]
    no_s19: bool,
    /// Skip the `.LST` output.
    #[arg(long = "no-lst")]
    no_lst: bool,

    /// `.bin` fill byte for unwritten addresses, hex (default FF).
    #[arg(long = "bin-fill", value_name = "HEX", default_value = "FF")]
    bin_fill: String,
    /// `.bin` window size in bytes, hex (default 40000 = 256 KB).
    #[arg(long = "bin-size", value_name = "HEX", default_value = "40000")]
    bin_size: String,
    /// `.bin` window base address, hex (default 0).
    #[arg(long = "bin-base", value_name = "HEX", default_value = "0")]
    bin_base: String,
}

/// Parse a hex integer, tolerating a `0x`/`$` prefix.
fn parse_hex(s: &str) -> Result<u32, String> {
    let t = s.trim().trim_start_matches("0x").trim_start_matches("0X").trim_start_matches('$');
    u32::from_str_radix(t, 16).map_err(|_| format!("invalid hex value: {s:?}"))
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let output_dir = cli
        .output_dir
        .clone()
        .or_else(|| cli.input.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    let (fill, size, base) = match (parse_hex(&cli.bin_fill), parse_hex(&cli.bin_size), parse_hex(&cli.bin_base)) {
        (Ok(f), Ok(s), Ok(b)) if f <= 0xFF && s > 0 => (f as u8, s, b),
        (Ok(f), _, _) if f > 0xFF => {
            eprintln!("error: --bin-fill must be a single byte (00-FF)");
            return ExitCode::from(2);
        }
        (Ok(_), Ok(0), _) => {
            eprintln!("error: --bin-size must be greater than 0");
            return ExitCode::from(2);
        }
        _ => {
            eprintln!("error: --bin-fill/--bin-size/--bin-base must be hex values");
            return ExitCode::from(2);
        }
    };

    let req = AssembleRequest {
        input: cli.input,
        output_dir,
        outputs: Outputs { obj: !cli.no_obj, s19: !cli.no_s19, lst: !cli.no_lst, bin: cli.bin },
        bin: BinOptions { fill, base, size },
    };
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
