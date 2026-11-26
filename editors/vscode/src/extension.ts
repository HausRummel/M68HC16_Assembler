// Starts the m68hc16-lsp language server for M68HC16 files. Highlighting and
// the problem matcher work without it; if the server can't be found, the
// extension says so once and carries on.

import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import { Executable, LanguageClient, LanguageClientOptions } from "vscode-languageclient/node";

let client: LanguageClient | undefined;

/** `m68hc16.serverPath`, else a binary shipped in the extension, else `PATH`. */
function serverCommand(ctx: vscode.ExtensionContext): string {
  const configured = vscode.workspace.getConfiguration("m68hc16").get<string>("serverPath", "").trim();
  if (configured) {
    return configured;
  }
  const exe = process.platform === "win32" ? "m68hc16-lsp.exe" : "m68hc16-lsp";
  const bundled = ctx.asAbsolutePath(path.join("server", exe));
  return fs.existsSync(bundled) ? bundled : exe;
}

async function start(ctx: vscode.ExtensionContext): Promise<void> {
  const cfg = vscode.workspace.getConfiguration("m68hc16");
  if (!cfg.get<boolean>("server.enable", true)) {
    return;
  }
  const command = serverCommand(ctx);
  const run: Executable = { command };
  const options: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "m68hc16" }],
    initializationOptions: { rootFiles: cfg.get<string[]>("rootFiles", []) },
    synchronize: { configurationSection: "m68hc16" },
  };
  const c = new LanguageClient("m68hc16", "M68HC16 Language Server", { run, debug: run }, options);
  try {
    await c.start();
    client = c;
  } catch (err) {
    void vscode.window.showWarningMessage(
      `M68HC16: could not start the language server "${command}" (${err}). ` +
        "Highlighting still works. Install m68hc16-lsp or set m68hc16.serverPath.",
    );
  }
}

async function restart(ctx: vscode.ExtensionContext): Promise<void> {
  const old = client;
  client = undefined;
  await old?.stop();
  await start(ctx);
}

export async function activate(ctx: vscode.ExtensionContext): Promise<void> {
  ctx.subscriptions.push(
    vscode.commands.registerCommand("m68hc16.restartServer", () => restart(ctx)),
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("m68hc16.serverPath") || e.affectsConfiguration("m68hc16.server.enable")) {
        void restart(ctx);
      }
    }),
  );
  await start(ctx);
}

export async function deactivate(): Promise<void> {
  await client?.stop();
}
