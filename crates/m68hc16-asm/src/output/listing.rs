//! MASM-faithful `.LST` assembly-listing writer.
//!
//! The listing has three parts: the source body (one row per source line), the
//! Symbol Table, and the Cross-Reference Table, paginated under a repeating page
//! header. This module is built incrementally; today it renders the **Symbol
//! Table** block, which is the byte-faithful name->address table other tooling
//! needs. Column layout was reverse-engineered from real MASM 4.6 output:
//!
//! ```text
//! symbol name       attrib.   section    value
//! -----------       -------   -------    -----
//! .asct             section   249        0x000f8000
//! BYTE1             .asct     249        0x0000200a
//! KONST             abs                  0x000000ff
//! ```
//!
//! Each row is `name  attrib  section  value` where the name field is min-16,
//! attrib min-8, section min-9, each followed by two spaces, then `0x` + 8 lower
//! hex digits. Section rows come first (in MASM's section-table order, supplied
//! by the caller), then program symbols in ASCII byte order (so uppercase sorts
//! before lowercase), then a `N symbols` footer counting sections + program syms.

use crate::encoder::{LineEmit, ListLine};
use crate::isa;
use crate::lexer::split_line;
use crate::symbols::{Kind, SymbolTable};

/// Render the listing **body**: one row per listed source line, in the form
///
/// ```text
/// Abs. Rel.   Loc    Obj. code   Source line
/// 1074   19i  000200 0000        PART_NUMBER_VECTOR_1  FDB  PAGE(PART_NUMBER)
/// ```
///
/// Column layout (0-based): `Abs`[0..4] ` ` `Rel`[5..9] (the include `i` suffix
/// sits at col 9) `  ` `Loc`[12..18] ` ` `Obj. code`[19..28] `   ` source from
/// col 31. Object bytes are grouped into big-endian 16-bit words; a line emitting
/// more than two words wraps onto continuation rows that repeat `Abs`/`Rel`,
/// advance `Loc`, and carry no source. `equ` shows its value as a 32-bit word
/// pair in `Obj. code` with a blank `Loc`. Source tabs expand at 8-column stops.
///
/// `line_emit` is indexed positionally by the assembled stream: its k-th entry is
/// the k-th `assembled` line in `list_lines`. (Page headers/pagination are added
/// by a separate layer; this renders the continuous body.)
pub fn body(list_lines: &[ListLine], line_emit: &[LineEmit]) -> String {
    let mut out = String::new();
    let mut asm_idx = 0usize;
    let rel_col = has_rel_column(list_lines);
    for (i, ll) in list_lines.iter().enumerate() {
        let emit = next_emit(ll, line_emit, &mut asm_idx);
        if !ll.listed {
            continue;
        }
        for row in line_rows(i + 1, ll.rel, ll.depth, emit, &ll.text, rel_col) {
            out.push_str(&row);
        }
    }
    out.push_str(&body_footer(list_lines.len()));
    out
}

/// Whether the listing carries the `Rel.` (per-file line number) column. MASM only
/// prints it when the source pulls in another file — i.e. some line comes from an
/// `INCLUDE` (depth > 0). A single flat file lists `Abs.` only.
fn has_rel_column(list_lines: &[ListLine]) -> bool {
    list_lines.iter().any(|ll| ll.depth > 0)
}

/// Take the next `LineEmit` for an `assembled` line, advancing the index. The
/// emit stream is positionally aligned with the `assembled` lines.
fn next_emit<'a>(ll: &ListLine, line_emit: &'a [LineEmit], asm_idx: &mut usize) -> Option<&'a LineEmit> {
    if ll.assembled {
        let e = line_emit.get(*asm_idx);
        *asm_idx += 1;
        e
    } else {
        None
    }
}

/// The blank line + `N lines assembled` tally that closes the body. MASM counts
/// the empty line after the file's final newline (which `lines()` drops), so the
/// tally is one past the highest `Abs`.
fn body_footer(n_lines: usize) -> String {
    // The count is right-justified in the 4-wide Abs column (wider numbers overflow
    // it naturally, as the Abs field itself does).
    format!("\r\n{:>4} lines assembled\r\n", n_lines + 1)
}

/// The physical `.LST` rows one source line contributes: one per object-code
/// chunk (object code wraps at two 16-bit words), or a single row otherwise.
/// `emit` is `None` for listed-but-not-assembled lines (INCLUDE / macro call).
fn line_rows(abs: usize, rel: u32, depth: u32, emit: Option<&LineEmit>, text: &str, rel_col: bool) -> Vec<String> {
    let isuf = if depth > 0 { 'i' } else { ' ' };
    // Prefix occupies cols 0..12 (wider when Abs/Rel exceed 4 digits). Without the
    // `Rel.` column (a flat, include-free source) the ` Rel.` field (5 cols) is
    // dropped, so the prefix is just `{abs:>4}` + the suffix slot + 2-space gap.
    let (pre, pre_cont) = if rel_col {
        // Instruction continuation blanks Rel; the abs field keeps its width and the
        // Rel+suffix+gap (8 cols) become spaces.
        (format!("{abs:>4} {rel:>4}{isuf}  "), format!("{abs:>4}        "))
    } else {
        let p = format!("{abs:>4}{isuf}  ");
        (p.clone(), p)
    };
    // MASM caps the source column at 132 chars (rest of a long comment dropped);
    // detab first since the cap is on displayed columns.
    let src: String = detab(text).chars().take(SRC_WIDTH).collect();

    let mut rows = Vec::new();
    match emit {
        Some(e) if !e.bytes.is_empty() => {
            let words = group_words(&e.bytes);
            let loc0 = e.loc.unwrap_or(0);
            let mut byte_off = 0u32;
            for (k, chunk) in words.chunks(2).enumerate() {
                let obj = chunk.join(" ");
                let (p, s) = if k == 0 {
                    (pre.as_str(), src.as_str())
                } else if e.is_data {
                    (pre.as_str(), "")
                } else {
                    (pre_cont.as_str(), "")
                };
                rows.push(fmt_row(p, Some(loc0 + byte_off), &obj, s));
                byte_off += chunk.iter().map(|w| (w.len() / 2) as u32).sum::<u32>();
            }
        }
        Some(e) if e.equ.is_some() => {
            let v = e.equ.unwrap();
            let obj = format!("{:04X} {:04X}", v >> 16, v & 0xFFFF);
            rows.push(fmt_row(&pre, None, &obj, &src));
        }
        Some(e) if e.loc.is_some() => rows.push(fmt_row(&pre, e.loc, "", &src)),
        _ => rows.push(fmt_row(&pre, None, "", &src)),
    }
    rows
}

/// Format one physical row. `loc` is the 6-hex `Loc` (blank if `None`); `obj` the
/// object-code text (≤9 chars, left-justified); `src` the source column. CRLF.
fn fmt_row(pre: &str, loc: Option<u32>, obj: &str, src: &str) -> String {
    let loc6 = match loc {
        Some(l) => format!("{l:06X}"),
        None => "      ".to_string(),
    };
    format!("{pre}{loc6} {obj:<9}   {src}\r\n")
}

/// Maximum width of the source column; longer source comments are cut here.
const SRC_WIDTH: usize = 132;

/// Group bytes into big-endian 16-bit words for the `Obj. code` column: each pair
/// renders as four hex digits, a trailing odd byte as two.
fn group_words(bytes: &[u8]) -> Vec<String> {
    bytes
        .chunks(2)
        .map(|c| {
            if c.len() == 2 {
                format!("{:02X}{:02X}", c[0], c[1])
            } else {
                format!("{:02X}", c[0])
            }
        })
        .collect()
}

/// Expand tabs to 8-column stops, measured from the source field's own column 0
/// (MASM detabs each source line before placing it in the listing).
fn detab(s: &str) -> String {
    if !s.contains('\t') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 8);
    let mut col = 0usize;
    for ch in s.chars() {
        if ch == '\t' {
            let n = 8 - (col % 8);
            out.extend(std::iter::repeat(' ').take(n));
            col += n;
        } else {
            out.push(ch);
            col += 1;
        }
    }
    out
}

/// The non-deterministic / configurable fields of a paginated listing.
pub struct PageOpts<'a> {
    /// Header filename for the top-level source. MASM uses the input file name as
    /// given on its command line (the corpus oracle renames it to `IN.ASM`).
    pub top_file: &'a str,
    /// Header time field. MASM stamps each page with the wall-clock at generation,
    /// so it is non-deterministic; we use one value for every page.
    pub timestamp: &'a str,
    /// Printed lines per page (`PLEN`; the corpus sets 60).
    pub plen: usize,
}

/// Lines in a page-header block: banner, file/time, title, blank, the two column
/// headers.
const HEADER_LINES: usize = 6;
/// Width of header line 1 (the `Page N` field is right-justified to this column).
const HEADER_WIDTH: usize = 79;

/// Render the paginated listing **body**: the [`body`] rows wrapped under repeating
/// page headers. `PAGE` directives force a break, `TTL` sets the running title, the
/// header filename tracks the current include, and a page breaks after `plen`
/// printed lines. (Everything is deterministic except the per-page timestamp.)
/// Returns the appended text and the final page number (so following sections —
/// the Symbol Table, Cross-Reference — continue the numbering).
pub fn paginate_body(list_lines: &[ListLine], line_emit: &[LineEmit], opts: &PageOpts) -> (String, usize) {
    let mut out = String::new();
    let mut asm_idx = 0usize;
    let mut page_no = 1usize; // page 1 is emitted eagerly below
    let rel_col = has_rel_column(list_lines);
    // Printed lines on the current page; set when the page-1 header is emitted below.
    let mut lines_on_page;
    let mut pending_break = false;
    // MASM emits the listing in pass 2 with the title already set from pass 1, so
    // page 1 shows the source's title even though `TTL` appears a few lines in.
    let mut title = last_ttl(list_lines).unwrap_or_default();
    // Include-stack of header filenames; `files[d]` is the file at depth `d`.
    let mut files: Vec<String> = vec![opts.top_file.to_string()];
    let mut last_include = String::new();
    // `PAGE` only ejects when the listing is on (nothing to eject under `NOLIST`),
    // so track listing state to ignore `PAGE` inside `NOLIST` equate includes.
    let mut listing_on = true;

    // MASM emits the page-1 header BEFORE processing any line (the first content
    // starts in the top file at depth 0). Emitting it eagerly — rather than lazily
    // on the first row — means a leading `PAGE` directive ejects to page 2, leaving
    // page 1 a header-only page (matches the oracle for files that open with PAGE).
    out.push_str(&page_header(page_no, opts.top_file, &title, opts.timestamp, Some(rel_col)));
    lines_on_page = HEADER_LINES;

    for (i, ll) in list_lines.iter().enumerate() {
        let want = ll.depth as usize + 1;
        while files.len() > want {
            files.pop();
        }
        if files.len() < want {
            files.push(last_include.clone());
        }
        let cur_file = files.last().cloned().unwrap_or_default();

        let emit = next_emit(ll, line_emit, &mut asm_idx);

        // Pagination-affecting directives, read from the source text. PAGE/TTL/
        // LIST/NOLIST act only when executed (`assembled`) — a copy inside a macro
        // *definition* is inert; INCLUDE is read regardless, for filename tracking.
        match split_line(&ll.text).op.map(|o| o.to_ascii_uppercase()).as_deref() {
            Some("INCLUDE") => last_include = include_name(&ll.text),
            Some("NOLIST") if ll.assembled => listing_on = false,
            Some("LIST") if ll.assembled => listing_on = true,
            Some("PAGE") if ll.assembled && listing_on => pending_break = true,
            Some("TTL") if ll.assembled => title = ttl_title(&ll.text).unwrap_or_default(),
            _ => {}
        }

        if !ll.listed {
            continue;
        }
        for row in line_rows(i + 1, ll.rel, ll.depth, emit, &ll.text, rel_col) {
            if pending_break || lines_on_page >= opts.plen {
                page_no += 1;
                out.push_str(&page_header(page_no, &cur_file, &title, opts.timestamp, Some(rel_col)));
                lines_on_page = HEADER_LINES;
                pending_break = false;
            }
            out.push_str(&row);
            lines_on_page += 1;
        }
    }
    out.push_str(&body_footer(list_lines.len()));
    (out, page_no)
}

/// Render the full paginated `.LST`: the body, then the Symbol Table, under one
/// continuous page numbering. (The Cross-Reference Table will append here too.)
pub fn listing(
    list_lines: &[ListLine],
    line_emit: &[LineEmit],
    symbols: &SymbolTable,
    macros: &[String],
    sections: &[(&str, u32)],
    asct: bool,
    opts: &PageOpts,
) -> String {
    let (mut out, page_no) = paginate_body(list_lines, line_emit, opts);
    let title = last_ttl(list_lines).unwrap_or_default();
    let rows = symbol_rows(symbols, macros, sections, asct);
    let (sym, page_no) = paginate_symbols(&rows, opts, &title, page_no);
    out.push_str(&sym);
    // The Cross-Reference Table (and the listing's trailing blank line that follows
    // it) appears only when there are program symbols to cross-reference; a file
    // with only section symbols (e.g. a comment-only source) ends at `N symbols`.
    if append_xref(&mut out, list_lines, line_emit, symbols, macros, &title, opts, page_no) {
        out.push_str("\r\n");
    }
    out
}

/// Paginate the Symbol Table rows under MASM's 4-line page headers (no `Abs. Rel.`
/// column row). The `Symbol Table:` + column intro prints once on the first symtab
/// page; the `N symbols` tally closes the last. Page numbering continues from
/// `page_no` (the body's final page). Returns the text and the final page number.
fn paginate_symbols(rows: &[String], opts: &PageOpts, title: &str, mut page_no: usize) -> (String, usize) {
    let rows_per_page = opts.plen.saturating_sub(HEADER_LINES); // 54
    let mut out = String::new();
    for (i, r) in rows.iter().enumerate() {
        if i % rows_per_page == 0 {
            page_no += 1;
            out.push_str(&page_header(page_no, opts.top_file, title, opts.timestamp, None));
            if i == 0 {
                out.push_str(&format!("Symbol Table:\r\n\r\n{SYM_COLHDR}\r\n{SYM_DASHES}\r\n"));
            }
        }
        out.push_str(r);
        out.push_str("\r\n");
    }
    out.push_str(&format!("\r\n{} symbols\r\n", rows.len()));
    (out, page_no)
}

/// Append the Cross-Reference Table to a full listing: for every program symbol
/// and macro (ASCII-sorted, no section symbols), the definition + reference lines.
/// Appends the Cross-Reference Table; returns whether any rows were emitted (false
/// when there are no program symbols, so the caller can drop the trailing blank).
fn append_xref(
    out: &mut String,
    list_lines: &[ListLine],
    line_emit: &[LineEmit],
    symbols: &SymbolTable,
    macros: &[String],
    title: &str,
    opts: &PageOpts,
    page_no: usize,
) -> bool {
    let rows = xref_rows(list_lines, line_emit, symbols, macros);
    let (x, _) = paginate_xref(&rows, opts, title, page_no);
    out.push_str(&x);
    !rows.is_empty()
}

/// One Cross-Reference entry: symbol name and its `(line, is_def)` entries sorted
/// descending. The def line renders with `@`; a macro has no def (only invocation
/// references). MASM counts each *occurrence* and lists the def even when the
/// symbol is also referenced on its own def line (`26204 @26204`).
struct XrefRow {
    name: String,
    entries: Vec<(u32, bool)>,
}

/// Collect each symbol's definition and reference Abs lines. A reference is a
/// symbol identifier in the OPERAND of an assembled line (each occurrence, at that
/// line's Abs); a macro is referenced where it is invoked (the op field of the
/// invocation line). Inherent (no-operand) instructions are skipped — their
/// "operand" field is actually a comment. Section symbols are excluded.
fn xref_rows(list_lines: &[ListLine], line_emit: &[LineEmit], symbols: &SymbolTable, macros: &[String]) -> Vec<XrefRow> {
    use std::collections::{HashMap, HashSet};
    let symset: HashSet<&str> = symbols.iter().map(|(n, _)| n.as_str()).collect();
    let macroset: HashSet<&str> = macros.iter().map(String::as_str).collect();
    // def value = (Abs, is_instruction). For a symbol referenced on its own def
    // line, MASM orders a data-directive def before the ref but an instruction def
    // after it; the flag drives that tiebreak.
    let mut def: HashMap<&str, (u32, bool)> = HashMap::new();
    let mut refs: HashMap<&str, Vec<u32>> = HashMap::new();

    let mut asm_idx = 0usize;
    for (i, ll) in list_lines.iter().enumerate() {
        let abs = (i + 1) as u32;
        // Whether MASM actually assembled this line (not a false-conditional skip).
        let processed = if ll.assembled {
            let pr = line_emit.get(asm_idx).map(|e| e.processed).unwrap_or(true);
            asm_idx += 1;
            pr
        } else {
            false
        };
        let p = split_line(&ll.text);
        if let Some(lbl) = p.label {
            if let Some(&canon) = symset.get(lbl) {
                let is_instr = p.op.is_some_and(|o| isa::lookup(o).is_some());
                def.entry(canon).or_insert((abs, is_instr));
            }
        }
        if let Some(op) = p.op {
            if let Some(&canon) = macroset.get(op) {
                // The invocation references the macro; its args are re-referenced
                // in the (assembled) expansion, so don't also scan the operand.
                refs.entry(canon).or_default().push(abs);
                continue;
            }
            // An inherent instruction takes no operand: the field is a comment.
            if isa::lookup(op).is_some_and(|d| d.modes.iter().all(|m| m.mode == isa::Mode::Inherent)) {
                continue;
            }
        }
        if processed {
            if let Some(operand) = p.operand {
                for id in operand_idents(operand) {
                    if let Some(&canon) = symset.get(id) {
                        refs.entry(canon).or_default().push(abs);
                    }
                }
            }
        }
    }

    let mut names: Vec<&str> = symset.iter().copied().chain(macroset.iter().copied()).collect();
    names.sort_unstable();
    names
        .into_iter()
        .map(|name| {
            // (line, tiebreak rank, is_def): for an equal line, a data-directive
            // def (0) sorts before refs (1), an instruction def (2) after them.
            let mut e: Vec<(u32, u8, bool)> =
                refs.get(name).map(|v| v.iter().map(|&l| (l, 1u8, false)).collect()).unwrap_or_default();
            if let Some(&(dv, is_instr)) = def.get(name) {
                e.push((dv, if is_instr { 2 } else { 0 }, true));
            }
            e.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            let entries = e.into_iter().map(|(l, _, d)| (l, d)).collect();
            XrefRow { name: name.to_string(), entries }
        })
        .collect()
}

/// Symbol-naming identifiers in an operand: skips `$hex`, `%bin`, decimal numbers,
/// and `'c'` char literals so their letters/digits are not mistaken for names.
fn operand_idents(operand: &str) -> Vec<&str> {
    let b = operand.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'$' => {
                i += 1;
                while i < b.len() && b[i].is_ascii_hexdigit() {
                    i += 1;
                }
            }
            b'%' => {
                i += 1;
                while i < b.len() && (b[i] == b'0' || b[i] == b'1') {
                    i += 1;
                }
            }
            b'\'' => {
                i += 1;
                while i < b.len() && b[i] != b'\'' {
                    i += 1;
                }
                i += usize::from(i < b.len());
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += 1;
                }
                i += usize::from(i < b.len());
            }
            b'0'..=b'9' => {
                while i < b.len() && b[i].is_ascii_alphanumeric() {
                    i += 1;
                }
            }
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => {
                let s = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                out.push(&operand[s..i]);
            }
            _ => i += 1,
        }
    }
    out
}

/// Paginate the Cross-Reference rows under MASM's 4-line headers. The `Cross
/// Reference Table:` intro prints once on the first page (and counts toward the
/// page). Each symbol's lines render at most 11 per physical row: the first as
/// `{name:<15}\t {…}`, continuations as `\t\t {…}`; the def line carries `@`. A
/// page holds `plen - 6` content lines (breaks may fall mid-symbol).
fn paginate_xref(rows: &[XrefRow], opts: &PageOpts, title: &str, mut page_no: usize) -> (String, usize) {
    let budget = opts.plen.saturating_sub(HEADER_LINES); // 54
    let mut out = String::new();
    let mut content = budget; // force a header before the first line
    let mut first_page = true;
    for r in rows {
        let toks: Vec<String> = r
            .entries
            .iter()
            .map(|&(l, is_def)| if is_def { format!("@{l}") } else { format!("{l}") })
            .collect();
        for (ci, chunk) in toks.chunks(11).enumerate() {
            let line = if ci == 0 {
                format!("{:<15}\t {}", r.name, chunk.join(" "))
            } else {
                format!("\t\t {}", chunk.join(" "))
            };
            if content >= budget {
                page_no += 1;
                out.push_str(&page_header(page_no, opts.top_file, title, opts.timestamp, None));
                content = 0;
                if first_page {
                    // The intro is extra — it does not count toward the row budget.
                    out.push_str("Cross Reference Table:\r\n\r\n");
                    first_page = false;
                }
            }
            out.push_str(&line);
            out.push_str("\r\n");
            content += 1;
        }
    }
    (out, page_no)
}

/// The page length from a `PLEN n` directive (lines per page). MASM's default
/// when no `PLEN` is given is 65 (verified against the BASE_RAM oracle: gold
/// form-feeds every 65 lines); jte sets `PLEN 60` explicitly.
pub fn page_length(list_lines: &[ListLine]) -> usize {
    list_lines
        .iter()
        .filter(|l| l.assembled)
        .find_map(|l| {
            let p = split_line(&l.text);
            if p.op.map(|o| o.eq_ignore_ascii_case("plen")).unwrap_or(false) {
                p.operand.and_then(|o| o.trim().parse::<usize>().ok())
            } else {
                None
            }
        })
        .unwrap_or(65)
}

/// A page-header block (CRLF), led by a form-feed except on page 1: banner with a
/// right-justified `Page N`, the file/timestamp line, the title, and a blank line.
/// The body adds the two column-header rows (`columns = Some(rel_col)`, with the
/// `Rel.` column present iff `rel_col`); the Symbol Table and Cross-Reference
/// sections pass `None`.
fn page_header(page_no: usize, file: &str, title: &str, timestamp: &str, columns: Option<bool>) -> String {
    const BANNER: &str = "Copyright 1993, Motorola Macro Assembler   Version 4.6";
    let page = format!("Page {page_no}");
    let pad = HEADER_WIDTH.saturating_sub(BANNER.len() + page.len());
    let ff = if page_no == 1 { "" } else { "\x0c" };
    let mut h = format!(
        "{ff}{BANNER}{:pad$}{page}\r\n68HC16 - {file} - {timestamp}\r\n{title}\r\n\r\n",
        "",
    );
    match columns {
        Some(true) => {
            h.push_str("Abs. Rel.   Loc    Obj. code   Source line\r\n");
            h.push_str("---- ----   ------ ---------   -----------\r\n");
        }
        Some(false) => {
            h.push_str("Abs.   Loc    Obj. code   Source line\r\n");
            h.push_str("----   ------ ---------   -----------\r\n");
        }
        None => {}
    }
    h
}

/// The value of the last `TTL` directive in the source (the title MASM carries
/// into page 1 from pass 1), if any.
fn last_ttl(list_lines: &[ListLine]) -> Option<String> {
    list_lines
        .iter()
        .rev()
        .filter(|l| l.assembled && split_line(&l.text).op.map(|o| o.eq_ignore_ascii_case("ttl")).unwrap_or(false))
        .find_map(|l| ttl_title(&l.text))
}

/// The title string of a `TTL "..."` directive (quotes stripped).
fn ttl_title(text: &str) -> Option<String> {
    split_line(text).operand.map(|o| o.trim().trim_matches('"').to_string())
}

/// The filename an `INCLUDE` directive names, as written (the header shows it
/// verbatim, e.g. `INITkjs.ASM`).
fn include_name(text: &str) -> String {
    split_line(text)
        .operand
        .and_then(|o| o.split_whitespace().next())
        .unwrap_or("")
        .trim_matches('"')
        .to_string()
}

/// The Symbol Table column header and its underline (shared by the block writer
/// and the paginated `.LST`).
const SYM_COLHDR: &str = "symbol name       attrib.   section    value";
const SYM_DASHES: &str = "-----------       -------   -------    -----";

/// The Symbol Table entry rows, each without a line terminator: the section rows
/// (in caller-supplied order) first, then program symbols and macros merged and
/// ASCII byte-sorted (uppercase before lowercase). Macros carry no value.
pub fn symbol_rows(symbols: &SymbolTable, macros: &[String], sections: &[(&str, u32)], asct: bool) -> Vec<String> {
    let mut rows = Vec::with_capacity(sections.len() + symbols.len() + macros.len());
    for (name, vaddr) in sections {
        rows.push(row(name, "section", sec_num(name), *vaddr));
    }
    let mac: std::collections::HashSet<&str> = macros.iter().map(String::as_str).collect();
    let mut names: Vec<&str> = symbols.iter().map(|(n, _)| n.as_str()).chain(mac.iter().copied()).collect();
    names.sort_unstable();
    for name in &names {
        if mac.contains(name) {
            rows.push(macro_row(name));
        } else {
            let (value, kind) = symbols.get_full(name).unwrap();
            let (attrib, secnum) = match kind {
                Kind::Abs => ("abs", ""),
                // A relocatable symbol's attrib is its containing section's name.
                // With `ASCT` every program symbol is in `.asct` (the jte case,
                // byte-exact); without it, it takes the content section holding its
                // address (BASE_RAM: `.bss`). No empty section exists in that mode,
                // so the highest section base not above the symbol is its section.
                Kind::Rel if asct => (".asct", "249"),
                Kind::Rel => {
                    let nm = sections
                        .iter()
                        .filter(|(_, v)| *v <= value as u32)
                        .max_by_key(|(_, v)| *v)
                        .map_or(".bss", |(n, _)| *n);
                    (nm, sec_num(nm))
                }
            };
            rows.push(row(name, attrib, secnum, value as u32));
        }
    }
    rows
}

/// The Symbol Table's `section` column for a section name: `.bss` registers as
/// type 0, `.asct` as 249 (the OBJ section-type registration; see masm-re-findings).
fn sec_num(name: &str) -> &'static str {
    if name == ".bss" {
        "0"
    } else {
        "249"
    }
}

/// Render the Symbol Table block (no surrounding page headers, LF endings) — the
/// byte-faithful name->address table other tooling needs. `sections` is the
/// `(name, vaddr)` of each emitted section in MASM symbol-table order.
pub fn symbol_table(symbols: &SymbolTable, macros: &[String], sections: &[(&str, u32)], asct: bool) -> String {
    let rows = symbol_rows(symbols, macros, sections, asct);
    let mut out = String::new();
    out.push_str("Symbol Table:\n\n");
    out.push_str(&format!("{SYM_COLHDR}\n{SYM_DASHES}\n"));
    for r in &rows {
        out.push_str(r);
        out.push('\n');
    }
    out.push_str(&format!("\n{} symbols\n", rows.len()));
    out
}

fn row(name: &str, attrib: &str, section: &str, value: u32) -> String {
    format!("{name:<16}  {attrib:<8}  {section:<9}  0x{value:08x}")
}

/// A macro's symbol-table row: name, attrib `macro`, no section/value. MASM pads
/// the line to column 40 (one past where a value would begin).
fn macro_row(name: &str) -> String {
    format!("{:<40}", format!("{name:<16}  macro"))
}

#[cfg(test)]
mod tests {
    use crate::encoder::{assemble_source, LineEmit, ListLine};

    fn ll(text: &str, rel: u32, depth: u32) -> ListLine {
        ListLine { text: text.to_string(), rel, depth, listed: true, assembled: true }
    }

    // Exercises every body rule: blank Loc/Obj on a comment; instruction object
    // code that wraps with a *blank* Rel continuation; data (fcb) object code that
    // wraps repeating Abs+Rel; the `equ` 32-bit value pair; an `rmb` Loc with no
    // Obj; and the include `i` suffix. Layout verified byte-exact on the full jte
    // listing; this pins the format so regressions surface fast.
    #[test]
    fn body_layout_and_wrapping() {
        let lines = [
            ll("* note", 1, 0),
            ll("  brclr foo", 2, 0),
            ll("  fcb 1,2,3,4,5,6", 3, 0),
            ll("K       equ  $FF", 4, 0),
            ll("R       rmb  4", 5, 0),
            ll("        nop", 9, 1),
        ];
        let emit = [
            LineEmit::default(),
            LineEmit { loc: Some(0x2000), bytes: vec![0x37, 0x2A, 0x80, 0x01, 0x10, 0x0A], ..Default::default() },
            LineEmit { loc: Some(0x2010), bytes: vec![1, 2, 3, 4, 5, 6], is_data: true, ..Default::default() },
            LineEmit { equ: Some(0x00FF), ..Default::default() },
            LineEmit { loc: Some(0x2020), ..Default::default() },
            LineEmit { loc: Some(0x2024), bytes: vec![0x27], ..Default::default() },
        ];
        let got = super::body(&lines, &emit);
        let want = concat!(
            "   1    1                      * note\r\n",
            "   2    2   002000 372A 8001     brclr foo\r\n",
            "   2        002004 100A        \r\n",
            "   3    3   002010 0102 0304     fcb 1,2,3,4,5,6\r\n",
            "   3    3   002014 0506        \r\n",
            "   4    4          0000 00FF   K       equ  $FF\r\n",
            "   5    5   002020             R       rmb  4\r\n",
            "   6    9i  002024 27                  nop\r\n",
            "\r\n   7 lines assembled\r\n",
        );
        assert_eq!(got, want);
    }

    #[test]
    fn paginate_symbols_structure() {
        let rows: Vec<String> = (0..5).map(|i| format!("SYM{i}")).collect();
        // plen 8 => 2 entry-rows per page; start after the body's page 10.
        let opts = super::PageOpts { top_file: "IN.ASM", timestamp: "TS ", plen: 8 };
        let (out, last) = super::paginate_symbols(&rows, &opts, "Title", 10);
        assert_eq!(last, 13); // 5 rows, 2/page -> pages 11,12,13
        assert!(out.starts_with('\u{0c}')); // form-feed (page != 1)
        assert_eq!(out.matches("Symbol Table:").count(), 1); // intro printed once
        assert!(out.contains("Page 11") && out.contains("Page 13"));
        assert!(!out.contains("Abs. Rel.")); // no body column header in symtab pages
        assert!(out.ends_with("\r\n5 symbols\r\n"));
    }

    #[test]
    fn operand_idents_skips_numeric_and_string_literals() {
        assert_eq!(super::operand_idents("$10,x"), vec!["x"]); // $hex skipped
        assert_eq!(super::operand_idents("PAGE(PART_NUMBER)"), vec!["PAGE", "PART_NUMBER"]);
        assert!(super::operand_idents("#$3E").is_empty()); // hex digits not idents
        assert_eq!(super::operand_idents("MODE-$F0000+7,MODE2"), vec!["MODE", "MODE2"]);
        assert!(super::operand_idents("\"boundary at FOO\"").is_empty()); // double-quoted
        assert!(super::operand_idents("'A'").is_empty()); // char literal
    }

    #[test]
    fn xref_wraps_at_11_and_marks_def() {
        // 13 lines, descending 12..0, the def at value 5.
        let entries: Vec<(u32, bool)> = (0..13u32).rev().map(|v| (v, v == 5)).collect();
        let row = super::XrefRow { name: "FOO".to_string(), entries };
        let opts = super::PageOpts { top_file: "IN.ASM", timestamp: "TS ", plen: 60 };
        let (out, _) = super::paginate_xref(&[row], &opts, "T", 0);
        let body: Vec<&str> = out.trim_end().split("\r\n").skip(4 + 2).collect(); // header + intro
        assert_eq!(body[0], "FOO            \t 12 11 10 9 8 7 6 @5 4 3 2"); // 11 entries, @ on def
        assert_eq!(body[1], "\t\t 1 0"); // continuation has the remaining 2
    }

    #[test]
    fn page_header_right_justifies_page_number() {
        let h = super::page_header(123, "F.ASM", "T", "TS ", None);
        let line1 = h.trim_start_matches('\u{0c}').lines().next().unwrap();
        assert_eq!(line1.len(), super::HEADER_WIDTH); // 79 cols
        assert!(line1.ends_with("Page 123"));
    }

    #[test]
    fn body_truncates_source_at_132() {
        let long = format!("* {}", "x".repeat(200));
        // A flat (include-free) source has no `Rel.` column, so the prefix is 26.
        let lines = [ll(&long, 1, 0)];
        let emit = [LineEmit::default()];
        let got = super::body(&lines, &emit);
        let first = got.lines().next().unwrap();
        assert_eq!(first.len(), 26 + 132); // no-Rel prefix (26) + 132-char source
        assert!(first.ends_with("xxx"));
        assert!(first.starts_with("   1   ")); // Abs only, no Rel field

        // With an include (depth > 0) the `Rel.` column returns -> prefix 31.
        let lines2 = [ll(&long, 1, 0), ll("  nop", 1, 1)];
        let emit2 = [LineEmit::default(), LineEmit::default()];
        let first2 = super::body(&lines2, &emit2).lines().next().unwrap().to_string();
        assert_eq!(first2.len(), 31 + 132);
    }


    // The Symbol Table MASM 4.6 emits for this exact source (captured from the
    // DOSBox oracle), modulo the section rows which the caller orders.
    const PROBE: &str = "        ASCT\n\
                         \x20       ORG  $2000\n\
                         CODE1   nop\n\
                         \x20       ldab #$12\n\
                         \x20       rts\n\
                         WORD1   fdb  $1234,$5678\n\
                         BYTE1   fcb  $56\n\
                         TABLE   equ  *\n\
                         \x20       fcb  $11,$22,$33\n\
                         RES1    rmb  4\n\
                         PASTRES equ  *\n\
                         KONST   equ  $00FF\n\
                         DERIVED equ  KONST+1\n\
                         \x20       end\n";

    #[test]
    fn symbol_table_matches_oracle() {
        let obj = assemble_source(PROBE);
        // Probe has one .asct (org $2000) + the always-present .bss; the symbol
        // table lists code/data sections first, .bss last.
        let sections = [(".asct", 0x2000u32), (".bss", 0u32)];
        let got = super::symbol_table(&obj.symbols, &[], &sections, obj.asct);
        let want = "\
Symbol Table:

symbol name       attrib.   section    value
-----------       -------   -------    -----
.asct             section   249        0x00002000
.bss              section   0          0x00000000
BYTE1             .asct     249        0x0000200a
CODE1             .asct     249        0x00002000
DERIVED           abs                  0x00000100
KONST             abs                  0x000000ff
PASTRES           .asct     249        0x00002012
RES1              .asct     249        0x0000200e
TABLE             .asct     249        0x0000200b
WORD1             .asct     249        0x00002006

10 symbols
";
        assert_eq!(got, want);
    }
}
