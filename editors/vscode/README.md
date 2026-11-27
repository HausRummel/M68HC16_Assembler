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
- **Live errors** as you type, from the real assembler run in memory by the
  `m68hc16-lsp` language server, on the right line of the right file, even inside
  `INCLUDE`d files and macro calls. Unsaved edits in any open file count.
- **Go to definition** (F12) for labels, equates and macros, across files.
- **Hover** on a symbol for its value, bank (`PK`) and offset, whether it is a
  constant or an address, and where it is defined. Hover on a mnemonic for its
  addressing modes and opcodes.
- **Build task errors** through the `$m68hc16` problem matcher, for Ctrl+Shift+B.

## Language server

The editor features above come from `m68hc16-lsp`, built from this repository
(`cargo build --release -p m68hc16-lsp`, or the Windows release download). The
extension looks for it in this order:

1. the `m68hc16.serverPath` setting,
2. a copy shipped inside the extension (`server/m68hc16-lsp[.exe]`),
3. `m68hc16-lsp` on your `PATH`.

If none is found, highlighting and the build task still work, and the extension
says once that the server is missing. **M68HC16: Restart Language Server** in the
Command Palette restarts it after you install or rebuild it.

### Which file gets assembled

An `INCLUDE`d fragment can't be assembled on its own, so the server assembles the
top-level file that pulls it in. By default that is any source in the workspace
that no other source includes; if several do, the one with the largest include
tree wins. To choose explicitly, set `m68hc16.rootFiles`:

```jsonc
// .vscode/settings.json
{ "m68hc16.rootFiles": ["src/main.asm"] }
```

A file that no root includes is assembled on its own, and an information note at
the top of the file says so when that leaves symbols undefined.

A full assemble of a large program takes under a second, so the server waits
until you pause typing (0.4 s) and then re-assembles in the background.

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

The build task and the language server report the same errors, so after a build
you may see each one twice (once per source) until the next edit; that is
expected. `m68hc16-asm` must be on your `PATH` (or give its full path as `command`), and the
output directory (`build` here) must already exist. Run the task from the workspace
root: the assembler prints paths relative to its working directory, which is how
the problem matcher finds the files.

## Settings

| Setting | Default | What |
|---|---|---|
| `m68hc16.rootFiles` | `[]` | top-level files to assemble (see above) |
| `m68hc16.serverPath` | `""` | path to `m68hc16-lsp` |
| `m68hc16.server.enable` | `true` | turn the language server off |
| `m68hc16.trace.server` | `off` | log LSP traffic to the Output panel |

The extension also sets these for M68HC16 files only. Override them in your own
settings under `"[m68hc16]"`:

| Setting | Default | Why |
|---|---|---|
| `files.encoding` | `cp437` | DOS sources; use `iso88591` if yours are Latin-1 |
| `editor.insertSpaces` | `true` | column alignment survives any tab width |
| `editor.detectIndentation` | `false` | keeps the above from being overridden per file |

If a file opens as **Plain Text** (no highlighting, hover or squiggles), a
`files.associations` setting or another extension has claimed it. Click the
language name in the status bar and pick **M68HC16 Assembly**, or pin it for the
workspace:

```jsonc
// .vscode/settings.json
{ "files.associations": { "*.asm": "m68hc16", "*.ASM": "m68hc16", "*.inc": "m68hc16" } }
```

The extension claims `.asm` and `.inc`, in any letter case. If you also edit other assembly languages,
map just your HC16 folders in workspace settings, for example
`"files.associations": { "**/hc16/**/*.asm": "m68hc16" }`, and set other folders
back to the language you use there.

## Install from source

```sh
cargo build --release -p m68hc16-lsp
cd editors/vscode
npm ci
mkdir -p server && cp ../../target/release/m68hc16-lsp server/   # optional: bundle the server
npx @vscode/vsce package          # writes m68hc16-asm-<version>.vsix
code --install-extension m68hc16-asm-*.vsix
```

A `.vsix` with a bundled server only works on that server's platform; package it
with `--target` (for example `vsce package --target win32-x64`) so the
Marketplace serves each platform its own build.

Or, while working on the grammar, open `editors/vscode` in VS Code and press F5 to
start an Extension Development Host.

## Roadmap

Find All References and autocomplete (labels, equates, macros, mnemonics).

## License

Apache-2.0, like the assembler.
