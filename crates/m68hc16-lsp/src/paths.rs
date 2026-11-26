//! Path handling shared by the server: lexical normalisation and the key used
//! to match a path from an `INCLUDE` against one from the editor.

use std::path::{Component, Path, PathBuf};

/// Remove `.` and resolve `..` lexically (`os/system/../fuel/X.ASM` becomes
/// `os/fuel/X.ASM`) without touching the file system.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Identity of a file for lookups. Windows paths compare case-insensitively
/// and with either slash, as the file system does.
pub fn key(p: &Path) -> String {
    let s = normalize(p).to_string_lossy().into_owned();
    if cfg!(windows) {
        s.replace('\\', "/").to_lowercase()
    } else {
        s
    }
}

/// Is this an assembly source by extension (`.asm` / `.inc`, any case)?
pub fn is_source(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("asm") || e.eq_ignore_ascii_case("inc"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_parent_segments() {
        assert_eq!(normalize(Path::new("/w/os/system/../fuel/./X.ASM")), PathBuf::from("/w/os/fuel/X.ASM"));
        assert_eq!(normalize(Path::new("../a/b")), PathBuf::from("../a/b"));
    }

    #[test]
    fn source_extensions() {
        assert!(is_source(Path::new("JTE.ASM")));
        assert!(is_source(Path::new("x.inc")));
        assert!(!is_source(Path::new("x.lst")));
    }
}
