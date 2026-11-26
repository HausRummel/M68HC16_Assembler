# M68HC16 Assembly for VS Code

Editor support for Motorola **M68HC16 (CPU16)** assembly in the MASM dialect, as
assembled by [`m68hc16-asm`](https://github.com/HausRummel/M68HC16_Assembler).

- **Syntax highlighting** for labels, all 285 CPU16 mnemonics, directives, macro calls,
  `$hex` / `%binary` / `@octal` / decimal numbers, `#` immediates, index registers,
  strings and `INCLUDE` paths.
- **MASM comment rules.** A line starting with `*` or `;` is a comment, and so is
  anything after the operand field, with or without a `;`
  (`INCLUDE io/LAMPS.ASM  * MIL LAMP` and `RTS  back to caller` both work).
- **DOS code page.** Files open as CP437 by default, so box-drawing comment headers
  from DOS-era sources show correctly, and saving keeps the original bytes.
- **Errors in the editor** through the `$m68hc16` problem matcher: build with
  Ctrl+Shift+B and errors land on the right line and column of the right file, even
  inside `INCLUDE`d files and macro calls.

## Build task

The extension does not run the assembler by itself. Add a task to the
workspace's `.vscode/tasks.json` and point it at the **top-level** source, the file
that `INCLUDE`s the others. Assembling an included fragment on its own would
report every symbol it borrows as undefined.

```jsonc
{
  "version": "2.0.0",
  "tasks": [
    {
      "label": "Assemble",
      "type": "process",
      "command": "m68hc16-asm",
      "args": ["src/main.asm", "--bin", "-o", "build"],
      "options": { "cwd": "${workspaceFolder}" },
      "problemMatcher": "$m68hc16",
      "group": { "kind": "build", "isDefault": true }
    }
  ]
}
```

`m68hc16-asm` must be on your `PATH` (or give its full path as `command`), and the
output directory (`build` here) must already exist. Run the task from the workspace
root: the assembler prints paths relative to its working directory, which is how
the problem matcher finds the files.

## Settings

The extension sets these for M68HC16 files only. Override them in your own
settings under `"[m68hc16]"`:

| Setting | Default | Why |
|---|---|---|
| `files.encoding` | `cp437` | DOS sources; use `iso88591` if yours are Latin-1 |
| `editor.insertSpaces` | `true` | column alignment survives any tab width |
| `editor.detectIndentation` | `false` | keeps the above from being overridden per file |

The extension claims `.asm` and `.inc`. If you also edit other assembly languages,
map just your HC16 folders in workspace settings, for example
`"files.associations": { "**/hc16/**/*.asm": "m68hc16" }`, and set other folders
back to the language you use there.

## Install from source

```sh
cd editors/vscode
npx @vscode/vsce package          # writes m68hc16-asm-<version>.vsix
code --install-extension m68hc16-asm-*.vsix
```

Or, while working on the grammar, open `editors/vscode` in VS Code and press F5 to
start an Extension Development Host.

## Roadmap

A language server (`crates/m68hc16-lsp`) built on the assembler's own parser, for
live diagnostics, go to definition, find references, and hover with resolved
addresses and `EQU` values.

## License

Apache-2.0, like the assembler.
