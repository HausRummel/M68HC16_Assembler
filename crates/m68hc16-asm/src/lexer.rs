//! Source-line splitter for the Motorola MASM dialect.
//!
//! Field rules observed in the corpus:
//! - A `*` in column 1 marks a whole-line comment; `;` starts a comment anywhere
//!   (but not inside a string).
//! - A label starts in column 1 and may end with `:`.
//! - The operation field follows (after the label, or after leading whitespace).
//! - The operand field is the remainder up to a `;` comment. Operands may contain
//!   commas (index/lists) and quoted strings (`fcc`).

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Line<'a> {
    pub label: Option<&'a str>,
    pub op: Option<&'a str>,
    pub operand: Option<&'a str>,
    pub comment: Option<&'a str>,
}

/// Split one source line into its fields.
pub fn split_line(line: &str) -> Line<'_> {
    // Whole-line comment: blank, or first non-blank is `*` or `;`.
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return Line::default();
    }
    if trimmed.starts_with('*') || trimmed.starts_with(';') {
        return Line { comment: Some(trimmed), ..Line::default() };
    }

    // Separate a trailing comment (`;` outside of single quotes).
    let (code, comment) = split_comment(line);

    let has_label = !code.starts_with([' ', '\t']);
    let mut rest = code.trim_start();

    let mut label = None;
    if has_label {
        let end = rest.find([' ', '\t']).unwrap_or(rest.len());
        let mut name = &rest[..end];
        name = name.strip_suffix(':').unwrap_or(name);
        label = Some(name);
        rest = rest[end..].trim_start();
    }

    let mut op = None;
    if !rest.is_empty() {
        let end = rest.find([' ', '\t']).unwrap_or(rest.len());
        op = Some(&rest[..end]);
        rest = rest[end..].trim_start();
    }

    let operand = if rest.is_empty() { None } else { Some(rest.trim_end()) };

    Line { label, op, operand, comment }
}

/// Split `line` into (code, comment) at the first `;` that is not inside a
/// single-quoted character/string literal.
fn split_comment(line: &str) -> (&str, Option<&str>) {
    let bytes = line.as_bytes();
    let mut in_quote = false;
    for (idx, &b) in bytes.iter().enumerate() {
        match b {
            b'\'' => in_quote = !in_quote,
            b';' if !in_quote => return (&line[..idx], Some(&line[idx..])),
            _ => {}
        }
    }
    (line, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_op_operand() {
        let l = split_line("start   ldaa  #$12   ; load");
        assert_eq!(l.label, Some("start"));
        assert_eq!(l.op, Some("ldaa"));
        assert_eq!(l.operand, Some("#$12"));
        assert_eq!(l.comment, Some("; load"));
    }

    #[test]
    fn no_label_when_indented() {
        let l = split_line("        rts");
        assert_eq!(l.label, None);
        assert_eq!(l.op, Some("rts"));
        assert_eq!(l.operand, None);
    }

    #[test]
    fn label_with_colon_and_equ() {
        let l = split_line("FOO:    equ   $40");
        assert_eq!(l.label, Some("FOO"));
        assert_eq!(l.op, Some("equ"));
        assert_eq!(l.operand, Some("$40"));
    }

    #[test]
    fn whole_line_comments() {
        assert_eq!(split_line("* a comment").comment, Some("* a comment"));
        assert_eq!(split_line("   ; indented comment").comment, Some("; indented comment"));
        assert_eq!(split_line("").op, None);
    }

    #[test]
    fn operand_with_commas_and_index() {
        let l = split_line("        ldaa  $10,x");
        assert_eq!(l.op, Some("ldaa"));
        assert_eq!(l.operand, Some("$10,x"));
    }

    #[test]
    fn semicolon_inside_quotes_is_not_a_comment() {
        let l = split_line("        fcb   ';'");
        assert_eq!(l.op, Some("fcb"));
        assert_eq!(l.operand, Some("';'"));
        assert_eq!(l.comment, None);
    }
}
