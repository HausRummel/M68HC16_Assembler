# MASM behavior specification

This directory captures the observed/documented behavior of Motorola's
MASM.exe so the new assembler can reproduce it bit-for-bit. Each document
should cite its source (manual page, MASM output sample, or Ghidra
disassembly address) so claims are auditable.

Planned documents (created during steps 2-8 of the build order):

- `directives.md` — every directive with grammar, examples, edge cases.
- `addressing-modes.md` — operand syntax → opcode/mode mapping.
- `expressions.md` — operator precedence, integer width, literals.
- `output-formats.md` — `.S19`, `.LST`, `.MAP` exact layouts.
