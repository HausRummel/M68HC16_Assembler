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
///
/// The operand field is a single whitespace-delimited token (quotes protect
/// internal spaces). Everything after it is the comment — this is how Motorola
/// MASM treats a trailing `* …` or `; …` comment after the operand.
pub fn split_line(line: &str) -> Line<'_> {
    // Whole-line comment: blank, or first non-blank is `*` or `;`.
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return Line::default();
    }
    if trimmed.starts_with('*') || trimmed.starts_with(';') {
        return Line { comment: Some(trimmed), ..Line::default() };
    }

    let col1_label = !line.starts_with([' ', '\t']);
    let mut rest = line.trim_start();

    // A label is a column-1 token, or any token ending in `:` (may be indented).
    let mut label = None;
    let first_end = rest.find([' ', '\t']).unwrap_or(rest.len());
    if col1_label || rest[..first_end].ends_with(':') {
        let name = rest[..first_end].strip_suffix(':').unwrap_or(&rest[..first_end]);
        label = Some(name);
        rest = rest[first_end..].trim_start();
    }

    // Operation: first token, unless what remains is a comment (`;` or `*`).
    let mut op = None;
    if !rest.is_empty() && !rest.starts_with([';', '*']) {
        let end = rest.find([' ', '\t']).unwrap_or(rest.len());
        op = Some(&rest[..end]);
        rest = rest[end..].trim_start();
    }

    // Operand (only meaningful with an op): first quote-aware token; the
    // remainder is the comment.
    let (mut operand, mut comment) = (None, None);
    if op.is_some() && !rest.is_empty() && !rest.starts_with(';') {
        let end = operand_end(rest);
        if end > 0 {
            operand = Some(&rest[..end]);
        }
        let after = rest[end..].trim_start();
        if !after.is_empty() {
            comment = Some(after);
        }
    } else if !rest.is_empty() {
        comment = Some(rest);
    }

    Line { label, op, operand, comment }
}

/// Index of the operand token's end: the first unquoted whitespace or `;`
/// (a `;` with no preceding space still starts the comment).
fn operand_end(s: &str) -> usize {
    let (mut sq, mut dq) = (false, false);
    for (i, &b) in s.as_bytes().iter().enumerate() {
        match b {
            b'\'' if !dq => sq = !sq,
            b'"' if !sq => dq = !dq,
            b' ' | b'\t' | b';' if !sq && !dq => return i,
            _ => {}
        }
    }
    s.len()
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

    #[test]
    fn star_after_operand_is_an_inline_comment() {
        let l = split_line("        INCLUDE FILE.ASM      * load equates");
        assert_eq!(l.op, Some("INCLUDE"));
        assert_eq!(l.operand, Some("FILE.ASM"));
        assert_eq!(l.comment, Some("* load equates"));
    }

    #[test]
    fn star_as_operand_is_the_location_counter() {
        let l = split_line("LABEL   equ *");
        assert_eq!(l.label, Some("LABEL"));
        assert_eq!(l.op, Some("equ"));
        assert_eq!(l.operand, Some("*"));
    }

    #[test]
    fn quoted_string_operand_keeps_spaces() {
        let l = split_line("        fail \"out of range here\"   * note");
        assert_eq!(l.op, Some("fail"));
        assert_eq!(l.operand, Some("\"out of range here\""));
        assert_eq!(l.comment, Some("* note"));
    }
}
