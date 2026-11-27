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
    /// Every textual occurrence of each symbol in those files: labels that
    /// define it, operands and macro calls that use it. Lines in false
    /// conditional blocks and macro bodies count too, as an editor search would.
    pub references: HashMap<String, Vec<SourceLoc>>,
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

    let names: HashSet<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    let mut references: HashMap<String, Vec<SourceLoc>> = HashMap::new();
    for f in &files {
        let Ok(text) = loader.load(f) else { continue };
        for (i, line) in text.lines().enumerate() {
            for (name, col) in line_symbols(line) {
                if names.contains(name) {
                    references.entry(name.to_string()).or_default().push(SourceLoc {
                        file: Some(f.clone()),
                        line: (i + 1) as u32,
                        col,
                    });
                }
            }
        }
    }

    Analysis { diagnostics: obj.diagnostics, symbols, files, references }
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '.' || c == '?'
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '?'
}

/// Candidate symbol names on a source line with their 1-based columns: the
/// label, the operation (a macro call), and identifiers in the operand. Skips
/// comments, quoted strings, numbers (`$1F`, `%101`, `@17`, `10`) and
/// built-in calls such as `PAGE(`.
pub fn line_symbols(line: &str) -> Vec<(&str, u32)> {
    let p = split_line(line);
    let mut out = Vec::new();
    for field in [p.label, p.op].into_iter().flatten() {
        out.push((field, encoder::field_col(line, Some(field))));
    }
    let Some(operand) = p.operand else { return out };
    let base = encoder::field_col(line, Some(operand)) - 1;
    let chars: Vec<(usize, char)> = operand.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (off, c) = chars[i];
        if c == '"' || c == '\'' {
            i += 1;
            while i < chars.len() && chars[i].1 != c {
                i += 1;
            }
            i += 1;
        } else if c.is_ascii_digit() || matches!(c, '$' | '%' | '@') {
            i += 1;
            while i < chars.len() && chars[i].1.is_ascii_alphanumeric() {
                i += 1;
            }
        } else if is_ident_start(c) {
            let start = i;
            while i < chars.len() && is_ident(chars[i].1) {
                i += 1;
            }
            let end = chars.get(i).map(|x| x.0).unwrap_or(operand.len());
            let followed_by_paren = chars.get(i).is_some_and(|x| x.1 == '(');
            if !followed_by_paren {
                let col = base as u32 + operand[..off].chars().count() as u32 + 1;
                out.push((&operand[chars[start].0..end], col));
            }
        } else {
            i += 1;
        }
    }
    out
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
