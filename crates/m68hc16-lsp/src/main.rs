//! `m68hc16-lsp`: a Language Server Protocol server for M68HC16 (CPU16)
//! assembly. It assembles the top-level file in memory with the real
//! assembler, so its errors, symbols and values are exactly the assembler's.
//! Speaks LSP over stdin/stdout.

mod discovery;
mod features;
mod paths;
mod server;

#[tokio::main]
async fn main() {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("m68hc16-lsp {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let (service, socket) = tower_lsp::LspService::new(server::Backend::new);
    tower_lsp::Server::new(tokio::io::stdin(), tokio::io::stdout(), socket).serve(service).await;
}
