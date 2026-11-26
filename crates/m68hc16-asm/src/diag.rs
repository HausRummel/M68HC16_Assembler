//! Diagnostics emitted during assembly.

use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Note,
    Warning,
    Error,
}

/// Where a diagnostic points in the original sources: the file it came from
/// (`None` = the top-level input) and a 1-based line and column within that
/// file. Columns count characters, which for the Latin-1 sources this
/// assembler reads equals the UTF-16 column editors expect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLoc {
    pub file: Option<PathBuf>,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    /// 1-based line in the flat, include- and macro-expanded stream the
    /// assembler consumes (what `HC16_LINES` dumps). Not a source-file line:
    /// use [`Diagnostic::loc`] for that.
    pub line: Option<u32>,
    /// Location in the original source file, when it is known.
    pub loc: Option<SourceLoc>,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>) -> Self {
        Self { severity: Severity::Error, message: message.into(), line: None, loc: None }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self { severity: Severity::Warning, message: message.into(), line: None, loc: None }
    }

    pub fn note(message: impl Into<String>) -> Self {
        Self { severity: Severity::Note, message: message.into(), line: None, loc: None }
    }

    pub fn with_line(mut self, line: u32) -> Self {
        self.line = Some(line);
        self
    }

    pub fn with_loc(mut self, loc: SourceLoc) -> Self {
        self.loc = Some(loc);
        self
    }

    pub fn is_error(&self) -> bool {
        matches!(self.severity, Severity::Error)
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.severity {
            Severity::Note => "note",
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        // With a file location, use the `file:line:col: kind: message` form that
        // compilers and editor problem matchers understand.
        match (&self.loc, self.line) {
            (Some(SourceLoc { file: Some(p), line, col }), _) => {
                write!(f, "{}:{line}:{col}: {kind}: {}", p.display(), self.message)
            }
            (Some(SourceLoc { file: None, line, col }), _) => {
                write!(f, "{kind} (line {line}, col {col}): {}", self.message)
            }
            (None, Some(n)) => write!(f, "{kind} (line {n}): {}", self.message),
            (None, None) => write!(f, "{kind}: {}", self.message),
        }
    }
}
