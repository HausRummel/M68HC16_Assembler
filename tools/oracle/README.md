# MASM golden oracle

Runs the **original Motorola MASM 4.6** toolchain under DOSBox to produce
known-good output (`OBJ` / `LST` / `S19`) for any HC16 source. This is the
ground-truth reference the new Rust assembler is validated against, byte-for-byte.

## Why

The new assembler must reproduce MASM's output exactly. Rather than guessing
encodings from a manual, we run the real assembler that built the original ECU
ROMs and diff against it. MASM is a DOS/4GW protected-mode program; DOSBox runs
it unmodified.

## Prerequisites

1. **DOSBox** (0.74-3 is fine) — runs the DOS/4GW binary.
2. The **original toolchain folder** containing `Masm.exe`, `Dos4gw.exe`,
   `Hex.exe`, `Ld.exe`.
3. Tell the scripts where those live. Pick one:
   - Copy `oracle.config.example.psd1` → `oracle.private.psd1` (gitignored) and
     edit the paths, **or**
   - set `$env:HC16_DOSBOX` and `$env:HC16_MASM_TOOLCHAIN`, **or**
   - pass `-DosBox` / `-Toolchain` explicitly.

> The toolchain path is deliberately kept out of git (it lives in an isolated
> location). Never hardcode it into a committed file — the `hooks/pre-commit`
> guard will reject it.

## Scripts

### `Invoke-MasmOracle.ps1`
Assembles one source through MASM + HEX in a hermetic temp dir and collects the
artifacts. Inside DOSBox the input is always `IN.ASM`, outputs are `OUT.*`
(DOSBox 0.74 is 8.3-filenames only — names don't affect the bytes).

```powershell
# Check the environment is wired up
.\Invoke-MasmOracle.ps1 -CheckEnv

# Assemble inline source, keep artifacts in .\out
.\Invoke-MasmOracle.ps1 -Source '        org $2000','        ldaa #$12','        end' -OutDir .\out

# See the generated DOSBox config without launching
.\Invoke-MasmOracle.ps1 -Source '...' -DryRun
```

The build mirrors the original `Asmb.bat`:
`MASM -a -x -t -o OUT.OBJ -l OUT.LST IN.ASM` then `HEX OUT.OBJ > OUT.S19`.

### `Get-MasmEncoding.ps1`
Assembles a body of source and parses the listing into structured rows
(`Abs`, `Loc`, `Bytes`, `Source`) — the authoritative encoding per line. Used to
build the ISA table and encoder fixtures.

```powershell
.\Get-MasmEncoding.ps1 -Body '        ldaa #$12','        addd #$1234' |
    Format-Table Loc, Bytes, Source
```

## Listing format (for `output/listing.rs`)

Fixed columns (`Abs.` 0–3, `Loc` 7–12, `Obj. code` 14–22, `Source` 26+):

```
Abs.   Loc    Obj. code   Source line
----   ------ ---------   -----------
   2   002000 7512        start   ldaa #$12
   3   002002 17F5 0040           ldab $40
```

Object code shows up to two hex words per line; longer encodings continue on the
next line. After `N lines assembled` come the `Symbol Table:` and
`Cross Reference Table:` sections, each repeated under a per-page banner
(`Copyright 1993, Motorola Macro Assembler   Version 4.6 ... Page N`). A full
captured example is in [`../../docs/spec/sample-listing.txt`](../../docs/spec/sample-listing.txt).

## Notes / caveats

- Single-file (snippet) assembly is hermetic. For full-module assembly with
  `include`s, pass `-IncludeDir` so the referenced sources are present.
- Outputs derived from **synthetic generic snippets** (no proprietary content)
  may be committed (e.g. `docs/spec/encoding-samples.tsv`). Outputs derived from
  the proprietary source corpus must stay under `tests/fixtures/private/`.
- A run launches a DOSBox window briefly; it auto-exits via the config's
  `[autoexec]`. A stuck build is killed after `-TimeoutSec` (default 60).
