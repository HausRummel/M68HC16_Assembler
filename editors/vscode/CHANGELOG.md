# Changelog

## 0.3.0

- Find All References, autocomplete (mnemonics, directives, macros, symbols with values), document outline and workspace symbol search, from `m68hc16-lsp`.
- `.asm` and `.inc` match in any letter case.

## 0.2.0

- Language client for `m68hc16-lsp`: live errors while typing (unsaved buffers included), go to definition, and hover with symbol values, bank and offset, and mnemonic addressing modes.
- Settings: `m68hc16.rootFiles`, `m68hc16.serverPath`, `m68hc16.server.enable`, `m68hc16.trace.server`; command **M68HC16: Restart Language Server**.
- Requires VS Code 1.82 or later.

## 0.1.0

- TextMate grammar for the CPU16 MASM dialect: labels, all 285 CPU16 mnemonics, directives, macro calls, numbers (`$hex`, `%binary`, `@octal`, decimal), immediates, index registers and trailing operand-field comments.
- `$m68hc16` problem matcher for `m68hc16-asm` output (`file:line:col: error: message`).
- Defaults `files.encoding` to `cp437` for M68HC16 files, so DOS box-drawing comment headers display correctly.
