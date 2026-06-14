# Survey: reference toolchain, ISA table, and corpus dialect

Initial reverse-engineering survey establishing what the new assembler must
reproduce. Sources are cited inline (file offsets are raw-file byte offsets in
`Masm.exe`).

## Reference toolchain

Located in an isolated local folder (the path is kept out of git; configured in
`tools/oracle/oracle.private.psd1` — see `tools/oracle/README.md`):

| Tool | Role |
|------|------|
| `Masm.exe` (200 KB) | Motorola **MASM 4.6** assembler — *Copyright (c) Motorola 1993*. WATCOM-C / DOS-4GW protected-mode binary. |
| `Dos4gw.exe` | DOS extender (loaded by the MZ stub). |
| `Ld.exe` | Linker (relocatable object → image). |
| `Hex.exe` | Object → Motorola S-record converter. |
| `Nm.exe`, `Ff.exe`, `I2m.exe` | Symbol/utility tools. |

Build invocation (from `Asmb.bat`):

```
MASM -a -x -t -o %1.OBJ -l %1.LST %1.ASM      ; -a absolute, -x xref, -l listing
HEX  %1.OBJ > %1.S19
```

Golden outputs present for validation: `JTE.OBJ`, `jte.S19`, `JTE.LS`, linker map
`01feb159.map`. (NOTE: the `JTE.LST` in the folder is a *failed 2026 re-assembly
with Alfred Arnold's "AS"*, not Motorola output — do not use as golden.)

Ghidra (MCP) additionally has real `68HC16:BE:24` ECU images loaded (e.g.
`f56041634ag.bin`) for encoding cross-checks.

## MASM operation table (instructions + directives)

MASM stores all recognized operations in one table inside the LE image:

- **Location:** raw offsets `0x2BD78`–`0x2E374`.
- **Record:** 22 bytes = 8-byte header + 14-byte null-padded lowercase name.
  - header `[0]` = handler/class byte (`b0`): 6–10 = instruction forms, 11/21/22 = directives.
  - header `[1]` = `0x01` (record signature).
  - header `[2..3]` (LE u16) = **`idx`**, index into the secondary opcode-descriptor table.
  - header `[4..5]` = `0x0000`.
  - header `[6..7]` (LE u16) = **`flags`**, addressing-mode / validity mask.
- **442 records** parsed (435 distinct names). Full dump: [`masm-mnemonic-table.tsv`](masm-mnemonic-table.tsv).

`flags` value distribution: `001F`×119, `0010`×111, `001C`×70, `0008`×36,
`0018`×30, `0003`×29, `0002`×20, `001E`×13, `000F`×10, plus a few singletons.
(Interpretation TBD — likely an addressing-mode-availability bitmask; directives
default to `001F`.)

**Note:** the secondary opcode-descriptor table (`idx` points into it) holds the
raw opcode bytes per mode. Decoding it from the binary is now a *cross-check*
rather than the primary path — the golden oracle (below) yields authoritative
encodings directly and far more reliably.

## Golden oracle (operational)

The original toolchain runs under DOSBox 0.74-3; see [`../../tools/oracle/`](../../tools/oracle).
`Get-MasmEncoding.ps1` assembles source through the real MASM and parses the
listing into `(Loc, Bytes, Source)` rows — authoritative CPU16 machine code.
Pipeline validated end-to-end: source → MASM → OBJ → HEX → S19, with listing
bytes == S19 data bytes.

### CPU16 encoding structure (oracle-derived sample)

Sample table in [`encoding-samples.tsv`](encoding-samples.tsv); reference listing
in [`sample-listing.txt`](sample-listing.txt). Observed opcode/prebyte structure:

| Addressing mode | Prebyte | Examples (mnemonic → bytes) |
|---|---|---|
| Immediate-8 | none | `ldaa #` →`75`, `ldab #` →`F5`, `adda #` →`71`, `anda #` →`76`, `cmpa #` →`78` |
| Immediate-16 | `37` | `addd #` →`37 B1`, `ldd #` →`37 B5`, `ldx #` →`37 BC`, `ldy #` →`37 BD` |
| Extended | `17`/`37` | `ldaa <ext>` →`17 75`, `staa <ext>` →`17 7A`, `ldd <ext>` →`37 F5` |
| Indexed (X/Y/Z) | none | `ldaa o,x` →`45`, `ldab o,x` →`C5`, `ldd o,y` →`95` |
| Relative (8) | none | `bra` →`B0`, `beq` →`B7`, `bne` →`B6` |
| Long branch (16) | `37` | `lbra` →`37 80` + 16-bit offset |
| Inherent | none/`37` | `rts` →`27 F7`, `clra` →`37 05`, `asld` →`27 F4`, `mul` →`37 24` |

The prebyte is selected by operand class (immediate width / mode), not by the
mnemonic alone — this is the core dispatch the encoder must implement.

**Next ISA step:** drive `Get-MasmEncoding.ps1` over a generated matrix of every
used mnemonic × every legal addressing mode to produce the complete encoding
table, then encode that as the Rust `isa` tables.

## Output formats

- **S-record** (`Hex.exe`): `S0` name record, `S1` data records (16-bit addr),
  `S9` terminator. See `output/srec.rs`.
- **Listing** (`-l`): fixed-column format documented in `sample-listing.txt`.
- **Object** (`OUT.OBJ`): relocatable; format TBD (decode for the linker stage).

## Corpus dialect usage

139 `.asm` files, 105,359 lines scanned (operation-field token frequency in
[`corpus-op-frequency.tsv`](corpus-op-frequency.tsv)):

- **244 distinct operations used**; **235** are real MASM ops (implement first),
  **200** MASM ops are unused by this corpus (lower priority; mostly HC05/08/11
  instructions and rare directives — but the conditional-assembly suite among
  them is still needed by macros).
- **Macro-defined pseudo-instructions** (must work before corpus assembles):
  `abx`, `aby`, `aba`, `ace` are HC11-style mnemonics synthesized via macros on
  the CPU16; `boundary` is the ORG-with-bounds-check macro from `MACROS.ASM`.
- **Top directives:** `fdb` (7172), `rmb` (5784), `equ` (3825), `fcb` (3007),
  `dc.w` (256), `page` (241), `include` (136).
- **Top instructions:** `ldaa, ldab, bra, brclr, staa, brset, bset, cmpa, jsr,
  bclr, ldx, ldd, bcc, beq, bne, bcs, clr, std, lde, stab, cmpb, lbra` …

### Dialect rules confirmed (see `MACROS.ASM`, MASM error strings)

- Comments: `*` in column 1 (whole line); `;` trailing.
- Macros: `NAME: macro` … `endm`; params `\1`, `\2`; `*` = location counter;
  `mlist on/off`; `fail "msg"`.
- Conditionals: `ifgt`/`ifc`/`ifnc`/`ifeq`… with `elsec` / `endc`.
- HC16 rules from MASM strings: "Direct page addressing is only valid with the
  68HC16", "code must be assembled at an even address", "even/quad alignment
  forced", "page boundary crossed".
