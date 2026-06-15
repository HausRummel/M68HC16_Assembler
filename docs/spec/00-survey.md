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

### Full ISA matrix (generated)

`Build-IsaMatrix.ps1` probes every instruction mnemonic × a battery of operand
templates through the oracle and records what assembles. Result:
[`isa-probe.tsv`](isa-probe.tsv) — **856 encodings across 215/215 instruction
mnemonics** (corpus-used MASM-table ops minus directives). Each row is
`mnemonic | mode(s) | bytes | nbytes` of authoritative MASM output.

Opcode-family structure observed across the matrix:
- 8-bit accumulators A/B and 16-bit D/E pick the **index register by opcode
  nibble**: X=`4x`, Y=`5x`, Z=`6x` for 8-bit-acc; `8x/9x/Ax` for 16-bit.
- **Prebyte** by operand class: `17` = 8-bit-acc extended / indexed-16; `37` =
  16-bit-D / immediate-16 / long-branch / inherent-ALU; `27` = E-offset indexed
  and many 1-byte inherents; no prebyte for immediate-8, 8-bit relative, indexed-8.
- `jmp`/`jsr` carry a **20-bit** address (bank nibble in byte 2: `7A 01 23 45`).
- Bit ops: `bset`=`39`, `bclr`=`38`, `brset`=`3B`, `brclr`=`3A` (`addr,#mask[,rel16]`).
- Register-list: `pshm`=`34`, `pulm`=`35` (2nd byte = register bitmask).
- Memory-move: `movb`=`37 FE`/`30`/`32`, `movw`=`37 FF`/`31`; `rmac`=`FB`.

The probe TEMPLATE is only a stimulus; the true mode is whatever the opcode
encodes (a non-branch op given `*` assembles as extended, etc.).

### Canonical ISA tables (generated)

`Build-IsaTable.ps1` *differentially* probes each mnemonic (two operand values per
mode) to split the constant opcode prefix from the operand bytes, then collapses
to canonical modes: 8/16/20-bit disambiguated by total length; PC-relative long
branches detected when the emitted operand ≠ the fed address; inherent ops by the
bare form; `pshm`/`pulm` register lists separated from E-indexing. Output:
[`isa-table.tsv`](isa-table.tsv) — 707 `(mnemonic, mode, prefix, operand_len)`
entries across 216 mnemonics.

`Generate-IsaRust.ps1` emits this as `crates/m68hc16-asm/src/isa/table.rs`
(`pub static INSTRUCTIONS: &[InsnDef]`). Hand-written `isa/mod.rs` defines `Mode`,
`IdxReg`, `ModeEntry`, `InsnDef`, and `lookup()`, with unit tests asserting key
encodings against the oracle bytes. `cargo test -p m68hc16-asm` is green.

### Assembler front end + encoder (implemented)

Implemented `lexer.rs` (line splitter), `expr.rs` (Pratt evaluator: `$ % @`
radixes, `*` location counter, symbols, `+ - * / % & | ^ << >>`), `symbols.rs`,
and `encoder.rs` — a fixpoint multi-pass driver that resolves the addressing mode
from the operand shape against the ISA table, emits `prefix` + operand bytes, and
handles `org/equ/fcb/fdb/fcc/rmb/end` + labels. `output/srec.rs` writes S-records;
`assemble()` is wired end-to-end (the CLI produces a `.S19`).

Three CPU16 behaviors pinned down against the oracle and baked in:
- **Relative displacements are taken from instruction-start + 6** (the 3-word
  prefetch pipeline), uniformly for short/long/bit branches — not the next instr.
- **Sections are padded to even length with a `0xFF` fill byte.**
- Immediate and indexed modes **auto-select 8- vs 16-bit by value** (matching MASM).

Validation: a 24-instruction snippet (`tests/fixtures/public/smoke.asm`) assembles
**byte-for-byte identical** to the real MASM S19 image (`smoke.bytes`), checked by
the `golden_fixtures` test. `cargo test` is green.

The instruction set is now complete for corpus usage, including register-list
(`pshm`/`pulm` — pull masks are the bit-reverse of push), memory-move
(`movb`/`movw`, X-indexed only), `rmac` (packed nibble offsets), and char
literals. Validated by a second golden fixture (`regmov.asm`/`.bytes`).

### Preprocessing: includes, macros, conditionals (implemented)

`encoder.rs` preprocesses once before the passes: recursive `include` splicing
(relative to each file's directory) and macro expansion (`NAME: macro`…`endm`,
`\1`..`\9` argument substitution). The passes evaluate conditional assembly
(`if/ifgt/iflt/ifge/ifle/ifeq/ifne/ifdef/ifndef/ifc/ifnc` with `else`/`elsec` and
`endc`/`endi`), handle `fail`, and treat listing/section/linkage directives
(`mlist`, `ttl`, `page`, `asct`, `xdef`, …) as no-ops for now.

Also added five inherent instructions the binary table-walk had skipped —
`abx/aba/aby/abz/ace` (e.g. `abx`=`37 4F`) — so the ISA table is now 221
mnemonics. Note: the corpus turned out to define exactly one macro (`BOUNDARY`)
and use conditionals only inside it; `abx/aba` are real instructions, not macros.

Validated by golden fixtures `prep.*` (macro + conditionals + `abx`/`aba`) and a
unit test for `include`.

### End-to-end gap-finding (in progress)

Running the assembler on the top-level corpus file surfaced (and fixed) several
real-world issues: `*` is an inline comment after the operand (the operand field
is one quote-aware token); sources carry extended-ASCII art in comments (read
lossily); `EVEN`/`longeven` emit `0xFF` fill to align; inherent-only instructions
ignore a trailing operand/comment. The whole include tree now resolves and real
code assembles.

The remaining big gap surfaced by the run: **indexed bit ops**. The corpus uses
the `,Z` addressing convention pervasively, including `bset addr,Z,#mask` →
`17 29 01 40` and `brset addr,Z,#mask,tgt` → `AB 01 40 FA` (indexed bit-branches
use **rel8**, extended use rel16). The differential classifier only probed the
extended bit form, so these per-register modes are missing from the ISA table —
the cause of most remaining errors (and, likely by cascade, the branch-range and
undefined-symbol errors, since a failed instruction shifts all later addresses).

Iterating the run drove the error count from ~12,400 to **0**: indexed bit-op
modes added (`bset/bclr/brset/brclr` × X/Y/Z; indexed bit-branches use rel8);
`PAGE(x)` built-in (bank byte = `(x>>16)&0xFF`); `EVEN`/`longeven` fill;
`EQU a,b` takes the first value; `FCB`/`FDB "str"` emit ASCII bytes; the lexer's
operand field is one quote-aware token ending at whitespace/`;`; indented `:`
labels; lossy reads; and — crucially — the fixpoint pass budget raised to 40 (a
25k-line file needs many passes to settle Ind8/Ind16 sizing, and an unconverged
layout produced thousands of spurious branch-range errors).

**The full top-level corpus file now assembles with 0 diagnostics** and emits a
~473 KB S-record image.

### Byte-exact diagnosis (root cause found)

A *fresh* oracle build of the current top file (MASM over the whole include tree in
DOSBox) confirms the reference image is authentic (429,492-byte S19, identical size
to the committed one). Our output assembles with **0 errors** and our
absolute-symbol addresses match MASM exactly — but the image is **not** byte-exact:
~88% of shared bytes differ, from accumulating layout drift.

Root cause (precisely diagnosed): MASM uses **16-bit indexed addressing for
relocatable (section-label) offsets even when the value fits 8 bits**, while we
shrink to 8-bit by value. E.g. `LDAA CLTEMP-RB,Z` (CLTEMP is a RAM label, offset
`0x91`) → MASM `17 65 0091` (Ind16); we emit `65 91` (Ind8). Absolute offsets (`equ`
constants, literals) size by value identically in both. The `-RB,Z` SRAM convention
is pervasive, so each shrink loses 2 bytes and the layout drifts (vectors/pointers
then mis-resolve). `HC16_SYMS=<file>` dumps our symbol table for comparison vs the
MASM listing.

**Operand sizing rule (implemented).** The actual rule is *forward-reference
commitment* (classic two-pass), not relocation: MASM uses the wide operand form
when an expression references a symbol defined **later** in the source (or
undefined), and otherwise sizes by value. Verified: `FWD_EQU,z` (forward `equ`) →
`Ind16`; `back2-back1,z` (both prior labels) → `Ind8`. Implemented via a
precomputed symbol→definition-line map and a `needs_wide()` check on indexed and
immediate operands. (A separate `Kind` Abs/Rel on each symbol is retained as
genuine relocation metadata for the eventual linker.)

**16-bit indexed bit ops (implemented).** A whole missing family: `bset/bclr/
brset/brclr addr,reg,#mask[,tgt]` with a 16-bit offset use bare opcodes
`08/09/0A/0B` (X), `18..1B` (Y), `28..2B` (Z) — distinct from the `17`-prefixed
8-bit forms — chosen by offset size + forward-ref.

**Progress (iterative).** Matching symbol addresses vs the MASM listing went
9,638 → 10,673 / 18,580 as these landed; each fix pushes the first drift point
later. The find-fix loop: dump our symbols (`HC16_SYMS=<file>`), diff against the
listing's symbol table, read the MASM listing at the first drift, probe the oracle
for the mis-sized instruction's true encoding, add the mode/rule. Remaining drift
(currently from ~`0x13DA0`) is more of the same.

**Full listing diff (key result).** A per-instruction byte-count diff of our output
vs the MASM listing (`HC16_TRACE=<file>` dumps `(byte-count, source)` per
instruction; MASM sizes come from listing address deltas — both robust to address
drift) shows **no missing instruction encodings remain**. Every one of the ~528
mismatches (out of ~22k aligned instructions) is `Ind8`↔`Ind16` or bit-branch
`8`↔`16` operand sizing — and it is **bidirectional** (e.g. `ldaa ours=4 masm=2`
*and* `ours=2 masm=4`).

The bidirectionality is diagnostic: our size decision uses the *converged* symbol
values, which drift; once a drifting offset crosses the 256 boundary the size flips
the wrong way, feeding back into more drift. MASM commits each operand's size in a
forward pass-1 (forward ref → wide; otherwise by the pass-1 value) and never
re-derives it. **So the one remaining task for byte-exactness is a MASM-faithful
size-commitment pass** — determine each span-dependent operand's size once in a
forward scan and hold it fixed, rather than recomputing from drift-prone values.
This is the classic span-dependent-instruction problem; the ISA itself is complete.

Then: that sizing pass, full sections/relocation + linker, listing/map output,
byte-exact S0/S9.

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
