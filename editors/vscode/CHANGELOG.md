# Changelog

## 0.1.0

- TextMate grammar for the CPU16 MASM dialect: labels, all 285 CPU16 mnemonics, directives, macro calls, numbers (`$hex`, `%binary`, `@octal`, decimal), immediates, index registers and trailing operand-field comments.
- `$m68hc16` problem matcher for `m68hc16-asm` output (`file:line:col: error: message`).
- Defaults `files.encoding` to `cp437` for M68HC16 files, so DOS box-drawing comment headers display correctly.
