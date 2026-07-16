# M68HC16 Assembler

A clean-room reimplementation of the **Motorola Macro Assembler (MASM) 4.6** for the
Motorola **M68HC16 (CPU16)**, written in Rust. It produces output that is
**byte-for-byte identical** to the original DOS toolchain.

The assembler has been validated end-to-end against the real Motorola MASM 4.6 +
`HEX.exe` (run under DOSBox) on complete ECU ROM builds: the emitted `.S19` and the
raw `.bin` (via `srec_cat`) match the toolchain's output exactly.

## What it produces

| Output | Description |
|--------|-------------|
| `.OBJ` | COFF relocatable object (MASM-compatible) |
| `.S19` | Motorola S-record (matches `HEX.exe`) |
| `.LST` | Paginated assembly listing (body + Symbol Table + Cross-Reference) |
| `.bin` | Raw flat ROM image (optional) |

## Prerequisites

- **Rust** (stable) — install from <https://rustup.rs>.

No other dependencies are required to build the CLI. The GUI uses
[`eframe`/`egui`](https://github.com/emilk/egui) (OpenGL via `glow`); on Linux you may
need the usual desktop dev libraries (X11/Wayland, OpenGL).

## Build (after clone / pull)

```sh
cargo build --release
```

The optimized binaries land in `target/release/`:

- `m68hc16-asm` — command-line assembler (`m68hc16-asm.exe` on Windows)
- `m68hc16-asm-gui` — desktop GUI (`m68hc16-asm-gui.exe` on Windows)

A debug build is just `cargo build`. Run the test suite with `cargo test`.

## Download (prebuilt binaries)

If you'd rather not build from source, grab a prebuilt Windows `x86_64` binary from
the [**Releases**](https://github.com/hausrummel/m68hc16_assembler/releases) page.
Each release attaches the CLI and GUI executables, a `THIRD-PARTY-LICENSES`
manifest, and a `SHA256SUMS` file — verify your download with
`sha256sum -c SHA256SUMS` (or `Get-FileHash` on Windows).

## Command-line usage

```sh
m68hc16-asm <input.asm> [-o <output-dir>] [options]
```

By default it writes `<stem>.OBJ`, `<stem>.S19`, and `<stem>.LST` next to the input
file (or into `-o <output-dir>`). The raw `.bin` is opt-in.

```
  -o, --output-dir <DIR>   Output directory (default: the input file's directory)
      --bin                Also write the raw binary image (<stem>.bin)
      --no-obj             Skip the .OBJ output
      --no-s19             Skip the .S19 output
      --no-lst             Skip the .LST output
      --bin-fill <HEX>     .bin fill byte for unwritten addresses   [default: FF]
      --bin-size <HEX>     .bin window size in bytes                [default: 40000]
      --bin-base <HEX>     .bin window base address                 [default: 0]
  -h, --help               Print help
  -V, --version            Print version
```

Examples:

```sh
# Full build (.OBJ + .S19 + .LST) plus a raw 256 KB ROM image
m68hc16-asm jte.asm --bin

# Just the S-record and binary, nothing else
m68hc16-asm jte.asm --bin --no-obj --no-lst

# A 128 KB image, zero-filled, for a different HC16 target
m68hc16-asm app.asm --bin --bin-size 20000 --bin-fill 00
```

### The `.bin` window

The raw binary is a fixed `[base, base + size)` window: every address nothing was
emitted into (gaps, reserved holes, trailing pad) is set to `fill`. The defaults —
**base 0, size `0x40000` (256 KB), fill `0xFF`** — produce a typical HC16 ROM image,
equivalent to:

```sh
srec_cat <stem>.S19 -fill 0xFF 0x00000 0x40000 -o <stem>.bin -binary
```

Because the window is a fixed device size (not auto-sized to where the data ends), the
image always matches the physical ROM capacity regardless of how much a given build
fills. Bytes that fall outside the window are dropped with a warning — enlarge
`--bin-size`/`--bin-base` if that happens.

## GUI usage

Launch `m68hc16-asm-gui`. Pick the input `.asm` and (optionally) an output directory,
tick which files to generate — **`.OBJ` `.S19` `.LST` `.BIN`** — and click **Assemble**.
When **.BIN** is ticked, **Fill / Base / Size** hex fields appear (defaulting to the
256 KB / `0xFF` window). The log lists every file written.

## Repository layout

```
crates/m68hc16-asm       the assembler library (lexer, parser, encoder, output writers)
crates/m68hc16-asm-cli   command-line front end
crates/m68hc16-asm-gui   egui desktop front end
docs/spec                ISA tables and encoding notes
tools/oracle             golden-oracle harness used to validate output byte-for-byte
```

## License

Licensed under the **Apache License, Version 2.0** — see [LICENSE](LICENSE) and
[NOTICE](NOTICE). Unless you state otherwise, any contribution you intentionally
submit for inclusion in this work shall be licensed as above, without additional
terms or conditions (Apache-2.0 §5).

Prebuilt binaries statically link third-party crates and embed fonts (under
permissive licenses such as MIT, BSD, Zlib, Unicode-3.0, OFL-1.1 and the Ubuntu
Font License). A complete `THIRD-PARTY-LICENSES` manifest is generated per release
and attached to each release archive; regenerate it locally with
[`cargo-about`](https://github.com/EmbarkStudios/cargo-about) via
`cargo about generate about.hbs -o THIRD-PARTY-LICENSES.html`.
