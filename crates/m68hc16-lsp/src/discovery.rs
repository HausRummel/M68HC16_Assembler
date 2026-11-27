//! Which top-level file assembles a given source.
//!
//! An included fragment cannot be assembled on its own (every symbol it borrows
//! would be undefined), so the server assembles the *root* that pulls it in.
//! Roots come from the `m68hc16.rootFiles` setting or, when that is empty, are
//! the workspace sources that no other source includes.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use m68hc16_asm::lexer::split_line;
use m68hc16_asm::SourceLoader;

use crate::paths::{is_source, key, normalize};

/// Directories never worth scanning.
const SKIP_DIRS: &[&str] = &["target", "node_modules", "build", "out", "dist"];
const MAX_DEPTH: usize = 16;
const MAX_FILES: usize = 20_000;

/// The workspace's sources and their `INCLUDE` edges.
#[derive(Debug, Default)]
pub struct Graph {
    /// Every source found, in scan order.
    pub files: Vec<PathBuf>,
    /// key -> files it includes (normalised paths).
    includes: HashMap<String, Vec<PathBuf>>,
    /// keys of files some other source includes.
    included: HashSet<String>,
}

impl Graph {
    /// Scan `folders` for sources and read their `INCLUDE` lines.
    pub fn scan(folders: &[PathBuf], loader: &dyn SourceLoader) -> Graph {
        let mut files = Vec::new();
        for f in folders {
            walk(f, 0, &mut files);
        }
        let mut g = Graph::default();
        for f in &files {
            let inc = includes_of(f, loader);
            for i in &inc {
                g.included.insert(key(i));
            }
            g.includes.insert(key(f), inc);
        }
        g.files = files;
        g
    }

    /// Sources nothing else includes, largest include tree first.
    pub fn roots(&self) -> Vec<PathBuf> {
        let mut r: Vec<(usize, PathBuf)> = self
            .files
            .iter()
            .filter(|f| !self.included.contains(&key(f)))
            .map(|f| (self.reachable(f).len(), f.clone()))
            .collect();
        r.sort_by(|a, b| b.0.cmp(&a.0));
        r.into_iter().map(|(_, f)| f).collect()
    }

    /// Keys of `root` and everything it includes, transitively.
    pub fn reachable(&self, root: &Path) -> HashSet<String> {
        let mut seen = HashSet::new();
        let mut q = VecDeque::from([normalize(root)]);
        while let Some(f) = q.pop_front() {
            let k = key(&f);
            if !seen.insert(k.clone()) {
                continue;
            }
            if let Some(next) = self.includes.get(&k) {
                q.extend(next.iter().cloned());
            }
        }
        seen
    }
}

/// Pick the root that assembles `file`: the first candidate whose include tree
/// contains it. `None` means no candidate does (assemble `file` on its own).
pub fn root_for(file: &Path, candidates: &[PathBuf], graph: &Graph) -> Option<PathBuf> {
    let k = key(file);
    candidates.iter().find(|r| graph.reachable(r).contains(&k)).map(|r| normalize(r))
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if p.is_dir() {
            if !name.starts_with('.') && !SKIP_DIRS.contains(&name) {
                walk(&p, depth + 1, out);
            }
        } else if is_source(&p) {
            out.push(normalize(&p));
        }
    }
}

/// The files `file` names in `INCLUDE` lines, resolved against its directory.
pub fn includes_of(file: &Path, loader: &dyn SourceLoader) -> Vec<PathBuf> {
    let Ok(text) = loader.load(file) else { return Vec::new() };
    let base = file.parent().unwrap_or(Path::new(""));
    text.lines()
        .filter_map(|l| {
            let p = split_line(l);
            if !p.op.is_some_and(|o| o.eq_ignore_ascii_case("include")) {
                return None;
            }
            let name = p.operand?.trim().trim_matches('"');
            Some(normalize(&base.join(name)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> PathBuf {
        let d = std::env::temp_dir().join(format!("hc16-lsp-disc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("os/sys")).unwrap();
        std::fs::create_dir_all(d.join("os/unused")).unwrap();
        std::fs::write(d.join("os/MAIN.ASM"), "        include sys/A.ASM\n").unwrap();
        std::fs::write(d.join("os/sys/A.ASM"), "        INCLUDE ../sys/B.ASM  * nested\n").unwrap();
        std::fs::write(d.join("os/sys/B.ASM"), "B:      nop\n").unwrap();
        std::fs::write(d.join("os/unused/OLD.ASM"), "        nop\n").unwrap();
        d
    }

    #[test]
    fn finds_roots_and_the_root_of_a_fragment() {
        let d = tree();
        let g = Graph::scan(&[d.clone()], &m68hc16_asm::FsLoader);
        let roots = g.roots();
        // MAIN pulls in two files, so it sorts before the lone OLD.ASM.
        assert_eq!(roots, vec![d.join("os/MAIN.ASM"), d.join("os/unused/OLD.ASM")]);
        assert_eq!(root_for(&d.join("os/sys/B.ASM"), &roots, &g), Some(d.join("os/MAIN.ASM")));
        assert_eq!(root_for(&d.join("os/unused/OLD.ASM"), &roots, &g), Some(d.join("os/unused/OLD.ASM")));
    }
}
