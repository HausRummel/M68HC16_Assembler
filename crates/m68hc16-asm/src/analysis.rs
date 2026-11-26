//! Whole-program analysis for editor integrations: diagnostics plus every
//! symbol's value and source location, without writing any output files.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::diag::{Diagnostic, SourceLoc};
use crate::encoder::{self, ListLine, SourceLoader};
use crate::lexer::split_line;
use crate::symbols::Kind;

/// What a name stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    /// An `equ`/`set` to a constant.
    Constant,
    /// A label, or an equate to an address expression.
    Address,
    /// A `macro` definition (no value).
    Macro,
}

#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub name: String,
    /// Final value; `None` for macros.
    pub value: Option<i64>,
    pub kind: SymbolKind,
    /// Where it is first defined (on a line that was actually assembled).
    /// `file` is always set.
    pub def: Option<SourceLoc>,
}

/// Result of [`analyze`].
#[derive(Debug, Default)]
pub struct Analysis {
    /// Every located diagnostic has `loc.file` set.
    pub diagnostics: Vec<Diagnostic>,
    pub symbols: Vec<SymbolInfo>,
    /// The root and every file it pulled in, in first-include order.
    pub files: Vec<PathBuf>,
}

impl Analysis {
    pub fn symbol(&self, name: &str) -> Option<&SymbolInfo> {
        self.symbols.iter().find(|s| s.name == name)
    }
}

/// Assemble `root` in memory (sources read through `loader`) and report what an
/// editor needs. Include paths resolve relative to each including file, as in
/// [`crate::assemble`].
pub fn analyze(root: &Path, loader: &dyn SourceLoader) -> Analysis {
    let src = match loader.load(root) {
        Ok(s) => s,
        Err(e) => {
            return Analysis {
                diagnostics: vec![Diagnostic::error(format!("cannot read {}: {e}", root.display()))],
                files: vec![root.to_path_buf()],
                ..Analysis::default()
            }
        }
    };
    let mut obj = encoder::assemble_source_with(&src, root.parent(), loader);
    let file_of = |f: &Option<PathBuf>| f.clone().unwrap_or_else(|| root.to_path_buf());

    for d in &mut obj.diagnostics {
        if let Some(loc) = d.loc.as_mut() {
            loc.file = Some(file_of(&loc.file));
        }
    }

    let mut files = vec![root.to_path_buf()];
    let mut seen: HashSet<PathBuf> = files.iter().cloned().collect();
    for ll in &obj.list_lines {
        let f = file_of(&ll.file);
        if seen.insert(f.clone()) {
            files.push(f);
        }
    }

    let defs = definition_locs(&obj.list_lines, &obj.line_emit, &file_of);
    let mut symbols: Vec<SymbolInfo> = obj
        .symbols
        .iter()
        .map(|(name, _)| {
            let (value, kind) = obj.symbols.get_full(name).expect("listed symbol");
            SymbolInfo {
                name: name.clone(),
                value: Some(value),
                kind: if kind == Kind::Abs { SymbolKind::Constant } else { SymbolKind::Address },
                def: defs.get(name.as_str()).cloned(),
            }
        })
        .collect();
    let macro_defs = macro_def_locs(&obj.list_lines, &file_of);
    for m in &obj.macros {
        symbols.push(SymbolInfo {
            name: m.clone(),
            value: None,
            kind: SymbolKind::Macro,
            def: macro_defs.get(m.as_str()).cloned(),
        });
    }
    symbols.sort_by(|a, b| a.name.cmp(&b.name));

    Analysis { diagnostics: obj.diagnostics, symbols, files }
}

/// First label definition of each name on a line MASM actually processed (a
/// label in a false conditional block defines nothing).
fn definition_locs<'a>(
    list_lines: &'a [ListLine],
    line_emit: &[encoder::LineEmit],
    file_of: &dyn Fn(&Option<PathBuf>) -> PathBuf,
) -> HashMap<&'a str, SourceLoc> {
    let mut out = HashMap::new();
    let mut asm_idx = 0usize;
    for ll in list_lines {
        if !ll.assembled {
            continue;
        }
        let processed = line_emit.get(asm_idx).map(|e| e.processed).unwrap_or(true);
        asm_idx += 1;
        if !processed {
            continue;
        }
        let p = split_line(&ll.text);
        if let Some(lbl) = p.label {
            out.entry(lbl).or_insert_with(|| SourceLoc {
                file: Some(file_of(&ll.file)),
                line: ll.rel,
                col: encoder::field_col(&ll.text, Some(lbl)),
            });
        }
    }
    out
}

fn macro_def_locs<'a>(
    list_lines: &'a [ListLine],
    file_of: &dyn Fn(&Option<PathBuf>) -> PathBuf,
) -> HashMap<&'a str, SourceLoc> {
    let mut out = HashMap::new();
    for ll in list_lines {
        let p = split_line(&ll.text);
        if p.op.is_some_and(|o| o.eq_ignore_ascii_case("macro")) {
            if let Some(lbl) = p.label {
                out.entry(lbl).or_insert_with(|| SourceLoc {
                    file: Some(file_of(&ll.file)),
                    line: ll.rel,
                    col: encoder::field_col(&ll.text, Some(lbl)),
                });
            }
        }
    }
    out
}
