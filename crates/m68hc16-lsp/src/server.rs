//! LSP plumbing: open documents, which root assembles each file, debounced
//! background analysis, and the request handlers.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use m68hc16_asm::{analyze, Analysis, FsLoader, SourceLoader};
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::discovery::{root_for, Graph};
use crate::features::{mnemonic_hover, severity_num, symbol_hover, token_len, word_at};
use crate::paths::{key, normalize};

/// Wait this long after the last keystroke before re-assembling. A full
/// firmware build takes most of a second, so running on every change would
/// queue work faster than it finishes.
const DEBOUNCE: Duration = Duration::from_millis(400);

const HINT_ALONE: &str = "Assembled on its own because no workspace file includes it. If it belongs to a larger \
build, add that build's top-level file to the m68hc16.rootFiles setting.";

/// Open documents' text (by path key), falling back to disk.
#[derive(Clone, Default)]
struct Overlay(Arc<HashMap<String, String>>);

impl SourceLoader for Overlay {
    fn load(&self, path: &Path) -> std::io::Result<String> {
        match self.0.get(&key(path)) {
            Some(t) => Ok(t.clone()),
            None => FsLoader.load(path),
        }
    }
}

struct Doc {
    path: PathBuf,
    text: String,
}

struct Root {
    path: PathBuf,
    generation: u64,
    analysis: Option<Arc<Analysis>>,
    /// Non-empty diagnostics last sent, per file, so only changes are re-sent.
    published: HashMap<Url, Vec<Diagnostic>>,
}

#[derive(Default)]
struct State {
    folders: Vec<PathBuf>,
    /// `m68hc16.rootFiles`, as given (absolute, or relative to the first folder).
    config_roots: Vec<String>,
    docs: HashMap<String, Doc>,
    roots: HashMap<String, Root>,
    /// file key -> root key that last assembled it.
    file_root: HashMap<String, String>,
    graph: Option<Graph>,
}

impl State {
    fn overlay(&self) -> Overlay {
        Overlay(Arc::new(self.docs.iter().map(|(k, d)| (k.clone(), d.text.clone())).collect()))
    }

    fn candidates(&self, graph: &Graph) -> Vec<PathBuf> {
        if self.config_roots.is_empty() {
            return graph.roots();
        }
        let base = self.folders.first().cloned().unwrap_or_default();
        self.config_roots
            .iter()
            .map(|r| {
                let p = Path::new(r);
                normalize(&if p.is_absolute() { p.to_path_buf() } else { base.join(p) })
            })
            .collect()
    }

    /// The root that assembles `file`, registering it if new.
    fn root_of(&mut self, file: &Path) -> String {
        let fk = key(file);
        if let Some(r) = self.file_root.get(&fk) {
            return r.clone();
        }
        let overlay = self.overlay();
        if self.graph.as_ref().is_none_or(|g| !g.files.iter().any(|f| key(f) == fk)) {
            self.graph = Some(Graph::scan(&self.folders, &overlay));
        }
        let graph = self.graph.as_ref().expect("scanned");
        let root = root_for(file, &self.candidates(graph), graph).unwrap_or_else(|| normalize(file));
        let rk = key(&root);
        self.roots.entry(rk.clone()).or_insert(Root {
            path: root,
            generation: 0,
            analysis: None,
            published: HashMap::new(),
        });
        self.file_root.insert(fk, rk.clone());
        rk
    }

    fn text_of(&self, path: &Path) -> Option<String> {
        match self.docs.get(&key(path)) {
            Some(d) => Some(d.text.clone()),
            None => FsLoader.load(path).ok(),
        }
    }
}

struct Inner {
    client: Client,
    state: Mutex<State>,
}

pub struct Backend(Arc<Inner>);

impl Backend {
    pub fn new(client: Client) -> Self {
        Backend(Arc::new(Inner { client, state: Mutex::new(State::default()) }))
    }
}

fn uri_path(uri: &Url) -> Option<PathBuf> {
    uri.to_file_path().ok().map(|p| normalize(&p))
}

fn root_files_from(v: &serde_json::Value) -> Option<Vec<String>> {
    let v = v.get("m68hc16").unwrap_or(v);
    let arr = v.get("rootFiles")?.as_array()?;
    Some(arr.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
}

/// Turn an analysis into per-file LSP diagnostics.
fn to_lsp(a: &Analysis, root: &Path, loader: &dyn SourceLoader) -> HashMap<Url, Vec<Diagnostic>> {
    let mut by_file: HashMap<Url, Vec<Diagnostic>> = HashMap::new();
    let mut texts: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for d in &a.diagnostics {
        let (path, line, col) = match &d.loc {
            Some(l) => (normalize(l.file.as_deref().unwrap_or(root)), l.line.saturating_sub(1), l.col.saturating_sub(1)),
            None => (normalize(root), 0, 0),
        };
        let lines = texts
            .entry(path.clone())
            .or_insert_with(|| loader.load(&path).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default());
        let len = lines.get(line as usize).map(|l| token_len(l, col as usize)).unwrap_or(1) as u32;
        let Ok(uri) = Url::from_file_path(&path) else { continue };
        by_file.entry(uri).or_default().push(Diagnostic {
            range: Range::new(Position::new(line, col), Position::new(line, col + len)),
            severity: Some(match severity_num(d.severity) {
                1 => DiagnosticSeverity::ERROR,
                2 => DiagnosticSeverity::WARNING,
                _ => DiagnosticSeverity::INFORMATION,
            }),
            source: Some("m68hc16-asm".into()),
            message: d.message.clone(),
            ..Diagnostic::default()
        });
    }
    // A fragment assembled alone reports everything it borrows as undefined;
    // say why once, at the top of the file.
    let alone = a.files.len() == 1
        && a.diagnostics.iter().any(|d| d.is_error() && d.message.starts_with("undefined symbol"));
    if alone {
        if let Ok(u) = Url::from_file_path(normalize(root)) {
            by_file.entry(u).or_default().insert(
                0,
                Diagnostic {
                    range: Range::new(Position::new(0, 0), Position::new(0, 1)),
                    severity: Some(DiagnosticSeverity::INFORMATION),
                    source: Some("m68hc16-lsp".into()),
                    message: HINT_ALONE.into(),
                    ..Diagnostic::default()
                },
            );
        }
    }
    by_file
}

impl Inner {
    /// Re-assemble root `rk` after `delay`, unless a newer request supersedes it.
    fn schedule(self: &Arc<Self>, rk: String, delay: Duration) {
        let generation = {
            let mut st = self.state.lock().unwrap();
            let Some(r) = st.roots.get_mut(&rk) else { return };
            r.generation += 1;
            r.generation
        };
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let (path, overlay) = {
                let st = me.state.lock().unwrap();
                match st.roots.get(&rk) {
                    Some(r) if r.generation == generation => (r.path.clone(), st.overlay()),
                    _ => return,
                }
            };
            let job = tokio::task::spawn_blocking(move || {
                let a = analyze(&path, &overlay);
                let diags = to_lsp(&a, &path, &overlay);
                (a, diags)
            });
            let Ok((analysis, diags)) = job.await else { return };
            let send: Vec<(Url, Vec<Diagnostic>)> = {
                let mut st = me.state.lock().unwrap();
                let fresh = match st.roots.get(&rk) {
                    Some(r) => r.generation == generation,
                    None => false,
                };
                if !fresh {
                    return;
                }
                for f in &analysis.files {
                    st.file_root.insert(key(f), rk.clone());
                }
                let r = st.roots.get_mut(&rk).expect("checked");
                let now: HashMap<Url, Vec<Diagnostic>> = diags.into_iter().filter(|(_, d)| !d.is_empty()).collect();
                let mut send: Vec<(Url, Vec<Diagnostic>)> =
                    r.published.keys().filter(|u| !now.contains_key(*u)).map(|u| (u.clone(), Vec::new())).collect();
                send.extend(now.iter().filter(|(u, d)| r.published.get(*u) != Some(*d)).map(|(u, d)| (u.clone(), d.clone())));
                r.published = now;
                r.analysis = Some(Arc::new(analysis));
                send
            };
            for (u, d) in send {
                me.client.publish_diagnostics(u, d, None).await;
            }
        });
    }

    /// The analysis covering `path` and the identifier at `pos`.
    fn lookup(&self, uri: &Url, pos: Position) -> Option<(Arc<Analysis>, String, usize)> {
        let path = uri_path(uri)?;
        let st = self.state.lock().unwrap();
        let rk = st.file_root.get(&key(&path))?;
        let a = st.roots.get(rk)?.analysis.clone()?;
        let text = st.text_of(&path)?;
        let line = text.lines().nth(pos.line as usize)?;
        let (w, start) = word_at(line, pos.character as usize)?;
        Some((a, w, start))
    }

    fn display_path(&self, p: &Path) -> String {
        let st = self.state.lock().unwrap();
        for f in &st.folders {
            if let Ok(rel) = p.strip_prefix(f) {
                return rel.display().to_string();
            }
        }
        p.display().to_string()
    }

    async fn reconfigure(self: &Arc<Self>, roots: Vec<String>) {
        let (clears, open): (Vec<Url>, Vec<PathBuf>) = {
            let mut st = self.state.lock().unwrap();
            if st.config_roots == roots {
                return;
            }
            st.config_roots = roots;
            let clears = st.roots.values().flat_map(|r| r.published.keys().cloned()).collect();
            st.roots.clear();
            st.file_root.clear();
            st.graph = None;
            (clears, st.docs.values().map(|d| d.path.clone()).collect())
        };
        for u in clears {
            self.client.publish_diagnostics(u, Vec::new(), None).await;
        }
        let mut rks = HashSet::new();
        for p in open {
            let rk = self.state.lock().unwrap().root_of(&p);
            rks.insert(rk);
        }
        for rk in rks {
            self.schedule(rk, Duration::ZERO);
        }
    }

    fn open_or_change(self: &Arc<Self>, uri: &Url, text: String, delay: Duration) {
        let Some(path) = uri_path(uri) else { return };
        let rk = {
            let mut st = self.state.lock().unwrap();
            st.docs.insert(key(&path), Doc { path: path.clone(), text });
            st.root_of(&path)
        };
        self.schedule(rk, delay);
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let mut st = self.0.state.lock().unwrap();
        #[allow(deprecated)]
        let folders: Vec<PathBuf> = match (&params.workspace_folders, &params.root_uri) {
            (Some(ws), _) if !ws.is_empty() => ws.iter().filter_map(|f| uri_path(&f.uri)).collect(),
            (_, Some(u)) => uri_path(u).into_iter().collect(),
            _ => Vec::new(),
        };
        st.folders = folders;
        if let Some(r) = params.initialization_options.as_ref().and_then(root_files_from) {
            st.config_roots = r;
        }
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo { name: "m68hc16-lsp".into(), version: Some(env!("CARGO_PKG_VERSION").into()) }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.0.client.log_message(MessageType::INFO, "m68hc16-lsp ready").await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, p: DidOpenTextDocumentParams) {
        self.0.open_or_change(&p.text_document.uri, p.text_document.text, Duration::ZERO);
    }

    async fn did_change(&self, p: DidChangeTextDocumentParams) {
        // Full sync: the last change holds the whole text.
        if let Some(c) = p.content_changes.into_iter().last() {
            self.0.open_or_change(&p.text_document.uri, c.text, DEBOUNCE);
        }
    }

    async fn did_save(&self, p: DidSaveTextDocumentParams) {
        // INCLUDE lines may have changed: rediscover roots next time one is needed.
        self.0.state.lock().unwrap().graph = None;
        let Some(path) = uri_path(&p.text_document.uri) else { return };
        let rk = self.0.state.lock().unwrap().root_of(&path);
        self.0.schedule(rk, Duration::ZERO);
    }

    async fn did_close(&self, p: DidCloseTextDocumentParams) {
        let Some(path) = uri_path(&p.text_document.uri) else { return };
        let rk = {
            let mut st = self.0.state.lock().unwrap();
            st.docs.remove(&key(&path));
            st.file_root.get(&key(&path)).cloned()
        };
        // Re-assemble from disk, in case the buffer was closed without saving.
        if let Some(rk) = rk {
            self.0.schedule(rk, Duration::ZERO);
        }
    }

    async fn did_change_configuration(&self, p: DidChangeConfigurationParams) {
        if let Some(r) = root_files_from(&p.settings) {
            self.0.reconfigure(r).await;
        }
    }

    async fn goto_definition(&self, p: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>> {
        let tp = p.text_document_position_params;
        let Some((a, w, _)) = self.0.lookup(&tp.text_document.uri, tp.position) else { return Ok(None) };
        let Some(def) = a.symbol(&w).and_then(|s| s.def.as_ref()) else { return Ok(None) };
        let Some(file) = def.file.as_deref() else { return Ok(None) };
        let Ok(uri) = Url::from_file_path(normalize(file)) else { return Ok(None) };
        let start = Position::new(def.line.saturating_sub(1), def.col.saturating_sub(1));
        let end = Position::new(start.line, start.character + w.chars().count() as u32);
        Ok(Some(GotoDefinitionResponse::Scalar(Location::new(uri, Range::new(start, end)))))
    }

    async fn hover(&self, p: HoverParams) -> Result<Option<Hover>> {
        let tp = p.text_document_position_params;
        let Some((a, w, start)) = self.0.lookup(&tp.text_document.uri, tp.position) else { return Ok(None) };
        let md = match a.symbol(&w) {
            Some(s) => {
                let def = s.def.as_ref().and_then(|d| {
                    d.file.as_deref().map(|f| format!("{}:{}", self.0.display_path(&normalize(f)), d.line))
                });
                symbol_hover(s, def)
            }
            None => match mnemonic_hover(&w) {
                Some(m) => m,
                None => return Ok(None),
            },
        };
        let line = tp.position.line;
        let range = Range::new(Position::new(line, start as u32), Position::new(line, (start + w.chars().count()) as u32));
        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value: md }),
            range: Some(range),
        }))
    }
}
