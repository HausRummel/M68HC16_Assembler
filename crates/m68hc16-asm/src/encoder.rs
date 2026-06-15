//! Instruction/operand encoder and the multi-pass assembly driver.
//!
//! Encoding is table-driven from [`crate::isa`]: the operand shape selects an
//! addressing [`Mode`], then the mode's `prefix` is emitted followed by the
//! operand bytes in that mode's layout. The driver iterates to a fixpoint so
//! forward references and operand-size selection converge.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::diag::Diagnostic;
use crate::expr::{self, EvalError};
use crate::isa::{self, IdxReg, InsnDef, Mode, ModeEntry};
use crate::lexer::split_line;
use crate::symbols::{Kind, SymbolTable};

/// Result of assembling a source: emitted `(address, byte)` pairs (in order),
/// the final symbol table, and diagnostics.
#[derive(Debug, Default)]
pub struct Object {
    pub data: Vec<(u32, u8)>,
    pub symbols: SymbolTable,
    pub diagnostics: Vec<Diagnostic>,
    /// Per-instruction `(address, byte-count, source)`, for diffing encodings
    /// against the MASM listing. The address lets a diff track layout drift (and
    /// localise size mismatches even through `nolist` blocks the listing omits).
    /// Dumped via `HC16_TRACE=<file>`.
    pub trace: Vec<(u32, u8, String)>,
    /// Addresses every `org` set the location counter to. They delimit output
    /// sections: a gap that an `org` jumped over is a section boundary (left
    /// empty), whereas a gap from `rmb`/`even` within a section is filled. See
    /// [`fill_sections`].
    pub org_targets: Vec<u32>,
    /// Per-emitted-item `(start, len, kind)` for everything that advances the
    /// location counter (instructions, data directives, reserves). Used by the
    /// COFF/OBJ writer to determine section extents/flags and per-symbol types.
    pub spans: Vec<(u32, u32, Elem)>,
    /// Program symbols in MASM symbol-table order: `(name, first-occurrence
    /// element context)`. The context drives the COFF symbol `type` (a label
    /// forward-referenced in an `fdb` is typed as a word). Populated after
    /// convergence.
    pub sym_order: Vec<(String, Option<Elem>)>,
}

/// The element kind of a span — drives the COFF symbol `type` (Code=0,
/// Word=T_SHORT=3, Byte=T_CHAR=2) and section flags (any Code -> TEXT, else data
/// -> DATA, else reserve-only -> BSS).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Elem {
    Code,
    Word,
    Byte,
    Reserve,
}

impl Object {
    /// The emitted bytes in order (addresses dropped) — handy for comparisons.
    pub fn bytes(&self) -> Vec<u8> {
        self.data.iter().map(|(_, b)| *b).collect()
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.is_error())
    }
}

/// Assemble a source string (no base directory, so `include` can't resolve files).
pub fn assemble_source(src: &str) -> Object {
    assemble_source_in(src, None)
}

/// Which role a pass plays in the two-phase (size-commit then emit) assembly.
enum Sizing<'a> {
    /// MASM pass 1: decide each span-dependent operand's width from
    /// backward-only knowledge and record it, keyed by source line.
    Record(&'a mut HashMap<u32, bool>),
    /// Emit passes: replay the frozen widths so the layout cannot drift.
    Use(&'a HashMap<u32, bool>),
}

/// Assemble a source string, resolving `include` files relative to `base_dir`.
/// Includes and macros are expanded once up front; the result is then assembled
/// in two phases, mirroring the original MASM:
///
/// 1. **Size commitment** (pass 1): a single forward scan builds the symbol table
///    *incrementally*, so a reference to a not-yet-defined symbol is a forward
///    reference (sized to the wide operand form) and a reference to an
///    already-defined symbol is sized by its — now stable — value. Each
///    span-dependent operand's chosen width is frozen.
/// 2. **Emit** (later passes): the frozen widths are replayed and only operand
///    *values* are resolved against the full symbol table. Because no size is
///    ever recomputed from a drifting value, the layout converges immediately
///    instead of oscillating (the classic span-dependent-instruction problem).
pub fn assemble_source_in(src: &str, base_dir: Option<&Path>) -> Object {
    let raw: Vec<String> = src.lines().map(str::to_string).collect();
    let lines = match preprocess(&raw, base_dir) {
        Ok(l) => l,
        Err(diags) => {
            return Object { diagnostics: diags, ..Object::default() };
        }
    };
    // First definition line of each symbol (1-based, matching run_pass `lineno`).
    // Used during pass 1 to flag forward references on operands (e.g. bit ops)
    // whose evaluator masks undefined-ness behind a zero placeholder.
    let def_line = definition_lines(&lines);

    // Phase 1: commit operand sizes in a single forward scan.
    let mut widths: HashMap<u32, bool> = HashMap::new();
    let pass1 = run_pass(&lines, &SymbolTable::new(), &def_line, Sizing::Record(&mut widths));

    // Phase 2: emit with widths frozen. Seed with pass 1's addresses so the very
    // first emit pass already has every label; iterate only to settle `equ`
    // chains. The layout is fixed, so this cannot drift.
    let mut symbols = pass1.symbols;
    let mut prev: Option<Vec<(u32, u8)>> = None;
    for _ in 0..40 {
        let mut obj = run_pass(&lines, &symbols, &def_line, Sizing::Use(&widths));
        if prev.as_ref() == Some(&obj.data) {
            // Convergence is on the *real* bytes; materialise the section fill on
            // the final image (MASM/HEX behaviour) only after the layout settles.
            obj.data = fill_sections(&obj.data, &obj.org_targets);
            obj.sym_order = order_symbols(&obj.symbols, &lines);
            if let Ok(path) = std::env::var("HC16_SYMCTX") {
                dump_sym_ctx(&obj, &lines, &path);
            }
            if let Ok(path) = std::env::var("HC16_SYMS") {
                use std::fmt::Write as _;
                let mut s = String::new();
                for (k, v) in obj.symbols.iter() {
                    let _ = writeln!(s, "{k}\t{v:X}");
                }
                let _ = std::fs::write(&path, s);
            }
            if let Ok(path) = std::env::var("HC16_TRACE") {
                use std::fmt::Write as _;
                let mut s = String::new();
                for (addr, n, src) in &obj.trace {
                    let _ = writeln!(s, "{addr:X}\t{n}\t{src}");
                }
                let _ = std::fs::write(&path, s);
            }
            return obj;
        }
        prev = Some(obj.data.clone());
        symbols = obj.symbols.clone();
    }
    let mut obj = run_pass(&lines, &symbols, &def_line, Sizing::Use(&widths));
    obj.data = fill_sections(&obj.data, &obj.org_targets);
    obj.sym_order = order_symbols(&obj.symbols, &lines);
    obj.diagnostics
        .push(Diagnostic::warning("assembly did not converge (possible phase error)"));
    obj
}

/// Program symbols in MASM symbol-table order, which is FIRST-OCCURRENCE order: a
/// symbol is entered when first *seen* — as a label OR in an operand — so a label
/// forward-referenced by the vector table sorts before its definition. Each
/// symbol also carries the element context of that first occurrence (the width of
/// the directive/instruction on that line), which sets its COFF `type`.
fn order_symbols(symbols: &SymbolTable, lines: &[String]) -> Vec<(String, Option<Elem>)> {
    let mut first: HashMap<String, (usize, Option<Elem>)> = HashMap::new();
    let mut seq = 0usize;
    for line in lines {
        let p = split_line(line);
        let ctx = op_elem(p.op);
        let mut note = |name: &str| {
            first.entry(name.to_string()).or_insert_with(|| {
                let s = seq;
                seq += 1;
                (s, ctx)
            });
        };
        if let Some(lbl) = p.label {
            note(lbl);
        }
        if let Some(operand) = p.operand {
            for id in identifiers(operand) {
                note(id);
            }
        }
    }
    let mut out: Vec<(usize, String, Option<Elem>)> = symbols
        .iter()
        .map(|(n, _)| {
            let (s, ctx) = first.get(n).copied().unwrap_or((usize::MAX, None));
            (s, n.clone(), ctx)
        })
        .collect();
    out.sort();
    out.into_iter().map(|(_, n, ctx)| (n, ctx)).collect()
}

/// Debug dump (env `HC16_SYMCTX`): one row per program symbol with the contexts
/// that could drive the COFF `type`, so the rule can be derived against the gold
/// OBJ. Columns: name, value(hex), kind, first-occ op, defining-line op,
/// element-at-address, whether referenced by any fdb / any fcb.
fn dump_sym_ctx(obj: &Object, lines: &[String], path: &str) {
    use std::fmt::Write as _;
    let def_line = definition_lines(lines);
    // first-occurrence op string + whether that first occurrence was as the
    // symbol's own label (definition) or as an operand (reference); + fdb/fcb refs.
    let mut first_op: HashMap<String, String> = HashMap::new();
    let mut first_lbl: HashMap<String, bool> = HashMap::new();
    let mut fdb_ref: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut fcb_ref: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in lines {
        let p = split_line(line);
        let opl = p.op.map(|s| s.to_ascii_lowercase()).unwrap_or_default();
        let mut note = |name: &str, as_label: bool| {
            if seen.insert(name.to_string()) {
                first_op.insert(name.to_string(), opl.clone());
                first_lbl.insert(name.to_string(), as_label);
            }
        };
        if let Some(lbl) = p.label {
            note(lbl, true);
        }
        if let Some(operand) = p.operand {
            for id in identifiers(operand) {
                note(id, false);
                if opl == "fdb" || opl == "dc.w" {
                    fdb_ref.insert(id.to_string());
                } else if opl == "fcb" || opl == "dc.b" || opl == "fcc" {
                    fcb_ref.insert(id.to_string());
                }
            }
        }
    }
    let addr_elem: HashMap<u32, Elem> =
        HashMap::from_iter(obj.spans.iter().map(|&(a, _, k)| (a, k)));
    let es = |e: Option<Elem>| match e {
        Some(Elem::Word) => "W",
        Some(Elem::Byte) => "B",
        Some(Elem::Reserve) => "R",
        Some(Elem::Code) => "C",
        None => "-",
    };
    let mut s = String::new();
    let _ = writeln!(s, "name\tvalue\tkind\tfirst_e\tflbl\tdef_e\tdef_op\taddr_e\tfdb\tfcb");
    for (name, value) in obj.symbols.iter() {
        let kind_s = match obj.symbols.get_full(name).map(|(_, k)| k) {
            Some(Kind::Abs) => "Abs",
            Some(Kind::Rel) => "Rel",
            None => "?",
        };
        let first_e = es(first_op.get(name).and_then(|o| op_elem(Some(o))));
        let flbl = first_lbl.get(name).copied().unwrap_or(false) as u8;
        let dop = def_line
            .get(name)
            .and_then(|&l| lines.get(l - 1))
            .map(|ln| split_line(ln).op.map(|s| s.to_ascii_lowercase()).unwrap_or_default())
            .unwrap_or_default();
        let def_e = es(op_elem(Some(&dop)));
        let ae = es(addr_elem.get(&(value as u32)).copied());
        let _ = writeln!(
            s,
            "{name}\t{value:X}\t{kind_s}\t{first_e}\t{flbl}\t{def_e}\t{dop}\t{ae}\t{}\t{}",
            fdb_ref.contains(name) as u8,
            fcb_ref.contains(name) as u8,
        );
    }
    let _ = std::fs::write(path, s);
}

/// The element width of an op: `fdb`->Word, byte directives->Byte, `rmb`->Reserve,
/// a known instruction->Code, anything else (`equ`, none, listing dirs)->None.
fn op_elem(op: Option<&str>) -> Option<Elem> {
    let op = op?.to_ascii_lowercase();
    match op.as_str() {
        "fdb" | "dc.w" => Some(Elem::Word),
        "fcb" | "dc.b" | "fcc" => Some(Elem::Byte),
        "rmb" | "ds" => Some(Elem::Reserve),
        _ if isa::lookup(&op).is_some() => Some(Elem::Code),
        _ => None,
    }
}

/// Identifiers (candidate symbol references) in `expr`, in order, skipping
/// numeric/char literals — mirrors [`needs_wide`]'s tokenizer.
fn identifiers(expr: &str) -> Vec<&str> {
    let b = expr.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'$' | b'%' | b'@' => {
                i += 1;
                while i < b.len() && b[i].is_ascii_alphanumeric() {
                    i += 1;
                }
            }
            c if c.is_ascii_digit() => {
                while i < b.len() && b[i].is_ascii_alphanumeric() {
                    i += 1;
                }
            }
            b'\'' => {
                i += 1;
                if i < b.len() {
                    i += 1;
                }
                if i < b.len() && b[i] == b'\'' {
                    i += 1;
                }
            }
            c if c.is_ascii_alphabetic() || c == b'_' || c == b'.' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                    i += 1;
                }
                if b.get(i) == Some(&b'(') {
                    continue; // function call name, not a symbol
                }
                out.push(&expr[start..i]);
            }
            _ => i += 1,
        }
    }
    out
}

/// First (1-based) line where each symbol is defined — a label or `equ`/`set`.
fn definition_lines(lines: &[String]) -> HashMap<String, usize> {
    let mut m = HashMap::new();
    for (i, line) in lines.iter().enumerate() {
        if let Some(lbl) = split_line(line).label {
            m.entry(lbl.to_string()).or_insert(i + 1);
        }
    }
    m
}

/// Does `expr` reference a symbol that is undefined or defined *after* `cur_line`?
/// Such forward references force the wide operand form. Identifiers immediately
/// followed by `(` are built-in functions (e.g. `PAGE`), not symbols.
///
/// Numeric literals are skipped wholesale: a `$`/`%`/`@`-prefixed or
/// leading-digit number can contain the letters `A`–`F` (e.g. `$3E`), which must
/// not be mistaken for an undefined symbol — that bug spuriously widened operands
/// like `ldab $3e,x`.
fn needs_wide(expr: &str, cur_line: u32, def_line: &HashMap<String, usize>) -> bool {
    let b = expr.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            // Hex/binary/octal literal: skip the radix mark and all its digits.
            b'$' | b'%' | b'@' => {
                i += 1;
                while i < b.len() && b[i].is_ascii_alphanumeric() {
                    i += 1;
                }
            }
            // Decimal literal: skip the whole run (no letters, but be uniform).
            c if c.is_ascii_digit() => {
                while i < b.len() && b[i].is_ascii_alphanumeric() {
                    i += 1;
                }
            }
            // Character literal `'x'`: skip the char and an optional closing quote.
            b'\'' => {
                i += 1;
                if i < b.len() {
                    i += 1;
                }
                if i < b.len() && b[i] == b'\'' {
                    i += 1;
                }
            }
            // Identifier: a symbol reference (unless it's a function call).
            c if c.is_ascii_alphabetic() || c == b'_' || c == b'.' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                    i += 1;
                }
                if b.get(i) == Some(&b'(') {
                    continue; // function call, not a symbol
                }
                match def_line.get(&expr[start..i]) {
                    Some(&d) if d as u32 <= cur_line => {} // defined at/before -> known
                    _ => return true,                      // forward or undefined -> wide
                }
            }
            _ => i += 1,
        }
    }
    false
}

// ---- Preprocessing: includes + macro expansion ------------------------------

/// Expand `include` files and macros into a flat line list. These are textual
/// transforms independent of assembly state, so they run once.
fn preprocess(raw: &[String], base: Option<&Path>) -> Result<Vec<String>, Vec<Diagnostic>> {
    let mut diags = Vec::new();
    let included = expand_includes(raw, base, 0, &mut diags);
    if !diags.is_empty() {
        return Err(diags);
    }
    let expanded = expand_macros(&included, &mut diags);
    if !diags.is_empty() {
        return Err(diags);
    }
    Ok(expanded)
}

fn expand_includes(lines: &[String], base: Option<&Path>, depth: u32, diags: &mut Vec<Diagnostic>) -> Vec<String> {
    if depth > 32 {
        diags.push(Diagnostic::error("include nesting too deep"));
        return Vec::new();
    }
    let mut out = Vec::new();
    for line in lines {
        let p = split_line(line);
        if p.op.map(|o| o.eq_ignore_ascii_case("include")).unwrap_or(false) {
            let Some(name) = p.operand.map(|s| s.trim().trim_matches('"')) else {
                diags.push(Diagnostic::error("include requires a filename"));
                continue;
            };
            let path = base.map(|b| b.join(name)).unwrap_or_else(|| PathBuf::from(name));
            match std::fs::read(&path) {
                Ok(bytes) => {
                    // Sources are DOS text with extended-ASCII art in comments; read
                    // lossily since only comments hold non-UTF-8 bytes.
                    let text = String::from_utf8_lossy(&bytes);
                    let sub: Vec<String> = text.lines().map(str::to_string).collect();
                    out.extend(expand_includes(&sub, path.parent(), depth + 1, diags));
                }
                Err(e) => diags.push(Diagnostic::error(format!("cannot include {}: {e}", path.display()))),
            }
        } else {
            out.push(line.clone());
        }
    }
    out
}

fn expand_macros(lines: &[String], diags: &mut Vec<Diagnostic>) -> Vec<String> {
    // Collect `NAME: macro` … `endm` definitions, removing them from the stream.
    let mut macros: HashMap<String, Vec<String>> = HashMap::new();
    let mut body: Vec<String> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;

    for line in lines {
        let p = split_line(line);
        let op = p.op.map(|o| o.to_ascii_lowercase());
        if let Some((name, lines)) = current.as_mut() {
            if op.as_deref() == Some("endm") {
                macros.insert(name.clone(), std::mem::take(lines));
                current = None;
            } else {
                lines.push(line.clone());
            }
            continue;
        }
        if op.as_deref() == Some("macro") {
            match p.label {
                Some(name) => current = Some((name.to_string(), Vec::new())),
                None => diags.push(Diagnostic::error("macro definition without a name")),
            }
            continue;
        }
        body.push(line.clone());
    }
    if current.is_some() {
        diags.push(Diagnostic::error("macro not closed with endm"));
    }

    expand_invocations(&body, &macros, diags, 0)
}

fn expand_invocations(lines: &[String], macros: &HashMap<String, Vec<String>>, diags: &mut Vec<Diagnostic>, depth: u32) -> Vec<String> {
    if depth > 64 {
        diags.push(Diagnostic::error("macro expansion too deep (recursion?)"));
        return Vec::new();
    }
    let mut out = Vec::new();
    for line in lines {
        let p = split_line(line);
        if let Some(op) = p.op {
            if let Some(mbody) = macros.get(op) {
                let args: Vec<String> = p
                    .operand
                    .map(|o| split_top_commas(o).iter().map(|s| s.trim().to_string()).collect())
                    .unwrap_or_default();
                let mut expanded = Vec::new();
                if let Some(lbl) = p.label {
                    expanded.push(format!("{lbl}:"));
                }
                for bl in mbody {
                    expanded.push(substitute_params(bl, &args));
                }
                out.extend(expand_invocations(&expanded, macros, diags, depth + 1));
                continue;
            }
        }
        out.push(line.clone());
    }
    out
}

/// Replace `\1`..`\9` with the corresponding macro argument (empty if absent).
fn substitute_params(line: &str, args: &[String]) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
            let n = chars[i + 1] as usize - '0' as usize;
            if n >= 1 && n <= args.len() {
                out.push_str(&args[n - 1]);
            }
            i += 2;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn run_pass(lines: &[String], sym_in: &SymbolTable, def_line: &HashMap<String, usize>, mut sizing: Sizing) -> Object {
    let mut out = Object::default();
    let mut lc: u32 = 0;
    let mut cond_stack: Vec<CondFrame> = Vec::new();

    // Symbols defined so far in *this* pass. In `Record` mode (pass 1) operand
    // values are read from this table, so a not-yet-defined symbol reads as a
    // forward reference; in `Use` mode they are read from `sym_in` (the full
    // table) and `defined` only accumulates this pass's labels for the next one.
    let mut defined = SymbolTable::new();
    let recording = matches!(sizing, Sizing::Record(_));
    // Pass-1 reads backward-only from `defined`; emit passes read the full table.
    macro_rules! read_syms {
        () => {
            if recording { &defined } else { sym_in }
        };
    }

    for (idx, raw_line) in lines.iter().enumerate() {
        let lineno = (idx + 1) as u32;
        let line = split_line(raw_line);
        let op_lower = line.op.map(|o| o.to_ascii_lowercase());

        // Conditional assembly: keep the stack balanced even in skipped regions.
        if let Some(op) = op_lower.as_deref() {
            if is_if_directive(op) {
                let pe = cond_emitting(&cond_stack);
                let cond = pe && eval_cond(op, line.operand, lc, read_syms!()).unwrap_or(false);
                cond_stack.push(CondFrame { parent_emit: pe, taken: cond, emitting: cond });
                continue;
            }
            if matches!(op, "else" | "elsec") {
                match cond_stack.last_mut() {
                    Some(f) => f.emitting = f.parent_emit && !f.taken,
                    None => err(&mut out, lineno, "else/elsec without a matching if".into()),
                }
                continue;
            }
            if matches!(op, "endc" | "endi" | "endif") {
                if cond_stack.pop().is_none() {
                    err(&mut out, lineno, "endc without a matching if".into());
                }
                continue;
            }
        }
        if !cond_emitting(&cond_stack) {
            continue;
        }

        // A label takes the current LC, except `equ`/`set` which take the operand.
        if let Some(lbl) = line.label {
            match op_lower.as_deref() {
                Some("equ") | Some("set") => {
                    if let Some(operand) = line.operand {
                        // MASM's `EQU a,b` takes the first value.
                        let first = split_top_commas(operand).first().map_or(operand, |s| s.trim());
                        match expr::eval_full(first, lc, read_syms!()) {
                            Ok((v, r)) => {
                                defined.define(lbl, v, if r == 0 { Kind::Abs } else { Kind::Rel })
                            }
                            Err(EvalError::Undefined(_)) => {} // resolves on a later pass
                            Err(EvalError::Syntax(s)) => err(&mut out, lineno, s),
                        }
                    } else {
                        err(&mut out, lineno, "equ requires an operand".into());
                    }
                }
                // A label is an address -> relocatable.
                _ => defined.define(lbl, lc as i64, Kind::Rel),
            }
        }

        let Some(op) = op_lower.as_deref() else { continue };

        // Listing/section/linkage directives that do not affect emitted bytes yet.
        if IGNORED_DIRECTIVES.contains(&op) {
            continue;
        }
        // `fail` forces an assembly error (used inside conditionals).
        if op == "fail" {
            let msg = line.operand.unwrap_or("").trim().trim_matches('"');
            err(&mut out, lineno, format!("fail: {msg}"));
            continue;
        }

        match op {
            "equ" | "set" => {} // handled with the label above
            "org" => match eval1(line.operand, lc, read_syms!()) {
                Ok(v) => {
                    lc = v as u32;
                    out.org_targets.push(lc);
                }
                Err(e) => err(&mut out, lineno, format!("org: {e}")),
            },
            // `rmb`/`even`/`longeven` only advance the location counter; the fill
            // bytes (if any) are materialised in `fill_sections`, which knows
            // whether a reserve sits between real data (filled) or is a leading/
            // trailing reserve or a reserve-only section (dropped, as MASM does).
            "rmb" | "ds" => match eval1(line.operand, lc, read_syms!()) {
                Ok(v) => {
                    if v > 0 {
                        out.spans.push((lc, v as u32, Elem::Reserve));
                    }
                    lc = lc.wrapping_add(v as u32);
                }
                Err(e) => err(&mut out, lineno, format!("rmb: {e}")),
            },
            "even" => {
                if lc & 1 != 0 {
                    out.spans.push((lc, 1, Elem::Reserve));
                    lc = lc.wrapping_add(1);
                }
            }
            "longeven" => {
                let start = lc;
                while lc & 3 != 0 {
                    lc = lc.wrapping_add(1);
                }
                if lc > start {
                    out.spans.push((start, lc - start, Elem::Reserve));
                }
            }
            "fcb" | "dc.b" => {
                let rt = read_syms!();
                let start = lc;
                emit_list(&mut out, &mut lc, lineno, line.operand, rt, 1);
                out.spans.push((start, lc - start, Elem::Byte));
            }
            "fdb" | "dc.w" => {
                let rt = read_syms!();
                let start = lc;
                emit_list(&mut out, &mut lc, lineno, line.operand, rt, 2);
                out.spans.push((start, lc - start, Elem::Word));
            }
            "fcc" => {
                let start = lc;
                emit_fcc(&mut out, &mut lc, lineno, line.operand);
                out.spans.push((start, lc - start, Elem::Byte));
            }
            "end" => break,
            _ => match isa::lookup(op) {
                Some(insn) => {
                    // Emit passes replay the committed width; pass 1 computes it.
                    let forced = match &sizing {
                        Sizing::Use(m) => m.get(&lineno).copied(),
                        Sizing::Record(_) => None,
                    };
                    let rt = read_syms!();
                    match encode_instruction(insn, line.operand, lc, rt, lineno, def_line, forced) {
                        Ok(enc) => {
                            if let Sizing::Record(m) = &mut sizing {
                                if let Some(w) = enc.width {
                                    m.insert(lineno, w);
                                }
                            }
                            for n in enc.unresolved {
                                err(&mut out, lineno, format!("undefined symbol \"{n}\""));
                            }
                            out.trace.push((lc, enc.bytes.len() as u8, raw_line.trim().to_string()));
                            out.spans.push((lc, enc.bytes.len() as u32, Elem::Code));
                            for b in enc.bytes {
                                out.data.push((lc, b));
                                lc = lc.wrapping_add(1);
                            }
                        }
                        Err(msg) => err(&mut out, lineno, msg),
                    }
                }
                None => err(&mut out, lineno, format!("unknown operation `{}`", line.op.unwrap())),
            },
        }
    }

    out.symbols = defined;
    out
}

/// Turn the emitted *real* data bytes into the final ROM image the way MASM /
/// HEX.exe does. The bytes are partitioned into sections at `org` boundaries; a
/// gap an `org` jumped over separates sections, while a gap from `rmb`/`even`
/// stays inside a section. For each section, the image runs from its first to
/// its last real byte with internal gaps filled `0xFF`, then padded so the
/// section ends on an even boundary. Leading/trailing reserves and reserve-only
/// sections (e.g. the RAM modules at `$f8xxx`) emit nothing.
fn fill_sections(data: &[(u32, u8)], org_targets: &[u32]) -> Vec<(u32, u8)> {
    let mut real: Vec<(u32, u8)> = data.to_vec();
    real.sort_by_key(|(a, _)| *a);
    real.dedup_by_key(|(a, _)| *a);
    if real.is_empty() {
        return real;
    }
    let targets: std::collections::BTreeSet<u32> = org_targets.iter().copied().collect();

    // Real bytes take precedence; fill/pad use `or_insert` so they never clobber.
    let mut img: std::collections::BTreeMap<u32, u8> = real.iter().copied().collect();

    let mut i = 0;
    while i < real.len() {
        // Extend the section while consecutive real bytes aren't split by an org.
        let mut j = i + 1;
        while j < real.len() {
            let (prev, cur) = (real[j - 1].0, real[j].0);
            let split = targets
                .range((std::ops::Bound::Excluded(prev), std::ops::Bound::Included(cur)))
                .next()
                .is_some();
            if split {
                break;
            }
            j += 1;
        }
        let (first, last) = (real[i].0, real[j - 1].0);
        for a in first..=last {
            img.entry(a).or_insert(0xFF); // fill internal rmb/even gaps
        }
        // Even-pad: if the next free address would be odd, MASM appends one 0xFF.
        if last % 2 == 0 {
            img.entry(last + 1).or_insert(0xFF);
        }
        i = j;
    }

    img.into_iter().collect()
}

fn err(out: &mut Object, line: u32, msg: String) {
    out.diagnostics.push(Diagnostic::error(msg).with_line(line));
}

fn eval1(operand: Option<&str>, lc: u32, sym: &SymbolTable) -> Result<i64, String> {
    let operand = operand.ok_or_else(|| "missing operand".to_string())?;
    expr::eval(operand, lc, sym).map_err(|e| e.to_string())
}

fn emit_list(out: &mut Object, lc: &mut u32, line: u32, operand: Option<&str>, sym: &SymbolTable, width: u32) {
    let Some(operand) = operand else {
        err(out, line, "directive requires an operand".into());
        return;
    };
    for item in split_top_commas(operand) {
        let item = item.trim();
        // A `"…"` string emits its ASCII bytes (both fcb and fdb).
        if item.len() >= 2 && item.starts_with('"') && item.ends_with('"') {
            for &b in item[1..item.len() - 1].as_bytes() {
                out.data.push((*lc, b));
                *lc = lc.wrapping_add(1);
            }
            continue;
        }
        match expr::eval(item, *lc, sym) {
            Ok(v) => push_be(out, lc, v, width),
            Err(EvalError::Undefined(_)) => push_be(out, lc, 0, width), // placeholder this pass
            Err(EvalError::Syntax(s)) => {
                err(out, line, s);
                push_be(out, lc, 0, width);
            }
        }
    }
}

fn emit_fcc(out: &mut Object, lc: &mut u32, line: u32, operand: Option<&str>) {
    let Some(operand) = operand else {
        err(out, line, "fcc requires a string".into());
        return;
    };
    let bytes = operand.as_bytes();
    if bytes.len() < 2 {
        err(out, line, "fcc: malformed string".into());
        return;
    }
    let delim = bytes[0];
    let mut i = 1;
    while i < bytes.len() && bytes[i] != delim {
        out.data.push((*lc, bytes[i]));
        *lc = lc.wrapping_add(1);
        i += 1;
    }
}

fn push_be(out: &mut Object, lc: &mut u32, value: i64, width: u32) {
    let v = value as u64;
    for shift in (0..width).rev() {
        out.data.push((*lc, (v >> (8 * shift)) as u8));
        *lc = lc.wrapping_add(1);
    }
}

/// Successful encoding of one instruction.
struct Enc {
    bytes: Vec<u8>,
    unresolved: Vec<String>,
    /// For a span-dependent operand (indexed / immediate / indexed bit op), the
    /// width chosen: `Some(true)` = wide form, `Some(false)` = narrow. `None` for
    /// operands whose size is fixed by the mnemonic. Recorded in pass 1, then
    /// replayed via `forced` in the emit passes so the layout cannot drift.
    width: Option<bool>,
}

/// Encode one instruction. `forced` replays a width committed in pass 1: `Some`
/// overrides the span-dependent size decision (the emit passes pass it), `None`
/// computes the size from `sym`/`def_line` (pass 1 does this).
fn encode_instruction(
    insn: &InsnDef,
    operand: Option<&str>,
    lc: u32,
    sym: &SymbolTable,
    cur_line: u32,
    def_line: &HashMap<String, usize>,
    forced: Option<bool>,
) -> Result<Enc, String> {
    let mut unresolved = Vec::new();

    // Inherent-only instructions ignore any operand — MASM does too (the operand
    // column often just holds a trailing comment the lexer captured).
    if insn.modes.iter().all(|m| matches!(m.mode, Mode::Inherent)) {
        if let Some(e) = mode_of(insn, |m| matches!(m, Mode::Inherent)) {
            return Ok(Enc { bytes: e.prefix.to_vec(), unresolved, width: None });
        }
    }

    // No operand -> inherent.
    let Some(raw) = operand.map(str::trim).filter(|s| !s.is_empty()) else {
        let e = mode_of(insn, |m| matches!(m, Mode::Inherent))
            .ok_or_else(|| format!("`{}` requires an operand", insn.mnemonic))?;
        return Ok(Enc { bytes: e.prefix.to_vec(), unresolved, width: None });
    };

    // Immediate. Ops with both 8- and 16-bit immediate forms take the smallest
    // that fits the value (matching MASM); an undefined value assumes the wider.
    if let Some(rest) = raw.strip_prefix('#') {
        let (v, undef) = match expr::eval(rest, lc, sym) {
            Ok(v) => (v, false),
            Err(EvalError::Undefined(n)) => {
                unresolved.push(n);
                (0, true)
            }
            Err(EvalError::Syntax(s)) => return Err(s),
        };
        let imm8 = mode_of(insn, |m| matches!(m, Mode::Imm8));
        let imm16 = mode_of(insn, |m| matches!(m, Mode::Imm16));
        // The 8-bit immediate is SIGNED — it is sign-extended into the (16-bit)
        // register, so MASM uses the 16-bit form once the value leaves
        // [-128, 127] (e.g. `adde #$80` = 128 -> 16-bit). This differs from an
        // indexed offset, which is an unsigned [0, 255] byte. An op with only one
        // immediate width is not span-dependent.
        let wide = match forced {
            Some(w) if imm8.is_some() && imm16.is_some() => w,
            _ => !(!undef && !needs_wide(rest, cur_line, def_line) && (-128..=127).contains(&v)),
        };
        let e = if !wide { imm8.or(imm16) } else { imm16.or(imm8) }
            .ok_or_else(|| format!("`{}` has no immediate mode", insn.mnemonic))?;
        let span_dep = imm8.is_some() && imm16.is_some();
        let mut bytes = e.prefix.to_vec();
        emit_be(&mut bytes, v, e.operand_len);
        return Ok(Enc { bytes, unresolved, width: span_dep.then_some(wide) });
    }

    // Bit-conditional branch: extended `addr,#mask,target` (rel16) or indexed
    // `off,reg,#mask,target` (rel8). The CPU16 prefetch makes rel = target-(lc+6).
    if mode_of(insn, |m| matches!(m, Mode::BitBrExt | Mode::BitBrInd(_) | Mode::BitBrInd16(_))).is_some() {
        let parts: Vec<&str> = split_top_commas(raw).iter().map(|s| s.trim()).collect();
        match parts.as_slice() {
            [addr, mask, tgt] => {
                let e = mode_of(insn, |m| matches!(m, Mode::BitBrExt))
                    .ok_or_else(|| format!("`{}`: no extended bit-branch", insn.mnemonic))?;
                let a = eval_or_zero(addr, lc, sym, &mut unresolved)?;
                let m = eval_or_zero(mask.trim_start_matches('#'), lc, sym, &mut unresolved)?;
                let rel = eval_or_zero(tgt, lc, sym, &mut unresolved)? - (lc as i64 + 6);
                let mut bytes = e.prefix.to_vec();
                bytes.push(m as u8);
                emit_be(&mut bytes, a, 2);
                emit_be(&mut bytes, rel, 2);
                return Ok(Enc { bytes, unresolved, width: None });
            }
            [off, reg, mask, tgt] if parse_reg(reg).is_some() => {
                let r = parse_reg(reg).unwrap();
                let o = eval_or_zero(off, lc, sym, &mut unresolved)?;
                let m = eval_or_zero(mask.trim_start_matches('#'), lc, sym, &mut unresolved)?;
                let rel = eval_or_zero(tgt, lc, sym, &mut unresolved)? - (lc as i64 + 6);
                // The 8-bit form carries (off8, rel8); the 16-bit form carries
                // (off16, rel16). MASM commits to the wide form if EITHER the
                // offset needs 16 bits OR the branch displacement does — and a
                // forward target's displacement is unknown in pass 1, so it
                // forces rel16 (hence the wide form) just like a forward offset.
                let wide = forced.unwrap_or_else(|| {
                    needs_wide(off, cur_line, def_line)
                        || !(0..=0xFF).contains(&o)
                        || needs_wide(tgt, cur_line, def_line)
                        || !(-128..=127).contains(&rel)
                });
                let mut bytes;
                if wide {
                    let e = mode_of(insn, |mm| matches!(mm, Mode::BitBrInd16(rr) if rr == r))
                        .ok_or_else(|| format!("`{}`: no 16-bit indexed bit-branch for {r:?}", insn.mnemonic))?;
                    bytes = e.prefix.to_vec();
                    bytes.push(m as u8);
                    emit_be(&mut bytes, o, 2);
                    emit_be(&mut bytes, rel, 2);
                } else {
                    let e = mode_of(insn, |mm| matches!(mm, Mode::BitBrInd(rr) if rr == r))
                        .ok_or_else(|| format!("`{}`: no indexed bit-branch for {r:?}", insn.mnemonic))?;
                    bytes = e.prefix.to_vec();
                    bytes.push(m as u8);
                    bytes.push(o as u8);
                    bytes.push(rel as u8);
                }
                return Ok(Enc { bytes, unresolved, width: Some(wide) });
            }
            _ => return Err(format!("`{}`: expects addr,#mask,target or off,reg,#mask,target", insn.mnemonic)),
        }
    }

    // Bit set/clear: extended `addr,#mask` or indexed `off,reg,#mask`.
    if mode_of(insn, |m| matches!(m, Mode::BitExt | Mode::BitInd(_) | Mode::BitInd16(_))).is_some() {
        let parts: Vec<&str> = split_top_commas(raw).iter().map(|s| s.trim()).collect();
        match parts.as_slice() {
            [addr, mask] => {
                let e = mode_of(insn, |m| matches!(m, Mode::BitExt))
                    .ok_or_else(|| format!("`{}`: no extended bit op", insn.mnemonic))?;
                let a = eval_or_zero(addr, lc, sym, &mut unresolved)?;
                let m = eval_or_zero(mask.trim_start_matches('#'), lc, sym, &mut unresolved)?;
                let mut bytes = e.prefix.to_vec();
                bytes.push(m as u8);
                emit_be(&mut bytes, a, 2);
                return Ok(Enc { bytes, unresolved, width: None });
            }
            [off, reg, mask] if parse_reg(reg).is_some() => {
                let r = parse_reg(reg).unwrap();
                let o = eval_or_zero(off, lc, sym, &mut unresolved)?;
                let m = eval_or_zero(mask.trim_start_matches('#'), lc, sym, &mut unresolved)?;
                let wide = forced.unwrap_or_else(|| needs_wide(off, cur_line, def_line) || !(0..=0xFF).contains(&o));
                let mut bytes;
                if wide {
                    let e = mode_of(insn, |mm| matches!(mm, Mode::BitInd16(rr) if rr == r))
                        .ok_or_else(|| format!("`{}`: no 16-bit indexed bit op for {r:?}", insn.mnemonic))?;
                    bytes = e.prefix.to_vec();
                    bytes.push(m as u8);
                    emit_be(&mut bytes, o, 2);
                } else {
                    let e = mode_of(insn, |mm| matches!(mm, Mode::BitInd(rr) if rr == r))
                        .ok_or_else(|| format!("`{}`: no indexed bit op for {r:?}", insn.mnemonic))?;
                    bytes = e.prefix.to_vec();
                    bytes.push(m as u8);
                    bytes.push(o as u8);
                }
                return Ok(Enc { bytes, unresolved, width: Some(wide) });
            }
            _ => return Err(format!("`{}`: expects addr,#mask or off,reg,#mask", insn.mnemonic)),
        }
    }

    // Register list (pshm/pulm): OR the per-register mask bits. pulm uses the
    // bit-reversed assignment of pshm so a push/pull pair restores order.
    if let Some(e) = mode_of(insn, |m| matches!(m, Mode::RegList)) {
        let pull = insn.mnemonic.eq_ignore_ascii_case("pulm");
        let mut mask = 0u8;
        for part in split_top_commas(raw) {
            let r = part.trim();
            mask |= reg_mask_bit(r, pull)
                .ok_or_else(|| format!("`{}`: unknown register `{r}`", insn.mnemonic))?;
        }
        let mut bytes = e.prefix.to_vec();
        bytes.push(mask);
        return Ok(Enc { bytes, unresolved, width: None });
    }

    // Memory move (movb/movw). Indexed forms support X only.
    if mode_of(insn, |m| matches!(m, Mode::MovMm | Mode::MovIdxExt | Mode::MovExtIdx)).is_some() {
        let parts: Vec<&str> = split_top_commas(raw).iter().map(|s| s.trim()).collect();
        match parts.as_slice() {
            [src, dst] => {
                let e = mode_of(insn, |m| matches!(m, Mode::MovMm)).unwrap();
                let s = eval_or_zero(src, lc, sym, &mut unresolved)?;
                let d = eval_or_zero(dst, lc, sym, &mut unresolved)?;
                let mut bytes = e.prefix.to_vec();
                emit_be(&mut bytes, s, 2);
                emit_be(&mut bytes, d, 2);
                return Ok(Enc { bytes, unresolved, width: None });
            }
            [off, reg, dst] if is_x_reg(reg) => {
                let e = mode_of(insn, |m| matches!(m, Mode::MovIdxExt))
                    .ok_or_else(|| format!("`{}`: no indexed-source move", insn.mnemonic))?;
                let o = eval_or_zero(off, lc, sym, &mut unresolved)?;
                let d = eval_or_zero(dst, lc, sym, &mut unresolved)?;
                let mut bytes = e.prefix.to_vec();
                emit_be(&mut bytes, o, 1);
                emit_be(&mut bytes, d, 2);
                return Ok(Enc { bytes, unresolved, width: None });
            }
            [src, off, reg] if is_x_reg(reg) => {
                let e = mode_of(insn, |m| matches!(m, Mode::MovExtIdx))
                    .ok_or_else(|| format!("`{}`: no indexed-dest move", insn.mnemonic))?;
                let s = eval_or_zero(src, lc, sym, &mut unresolved)?;
                let o = eval_or_zero(off, lc, sym, &mut unresolved)?;
                let mut bytes = e.prefix.to_vec();
                emit_be(&mut bytes, o, 1);
                emit_be(&mut bytes, s, 2);
                return Ok(Enc { bytes, unresolved, width: None });
            }
            _ => return Err(format!("`{}`: bad move operand (index register must be X)", insn.mnemonic)),
        }
    }

    // rmac: two signed offsets packed into one byte (high nibble, low nibble).
    if let Some(e) = mode_of(insn, |m| matches!(m, Mode::Mac)) {
        let parts = split_top_commas(raw);
        if parts.len() != 2 {
            return Err(format!("`{}` expects two offsets", insn.mnemonic));
        }
        let a = eval_or_zero(parts[0].trim(), lc, sym, &mut unresolved)?;
        let b = eval_or_zero(parts[1].trim(), lc, sym, &mut unresolved)?;
        let mut bytes = e.prefix.to_vec();
        bytes.push(((a as u8 & 0x0F) << 4) | (b as u8 & 0x0F));
        return Ok(Enc { bytes, unresolved, width: None });
    }

    // Indexed or accumulator-E indexed: `<base>,<reg>`.
    if let Some((base, reg)) = split_index(raw) {
        if base.eq_ignore_ascii_case("e") {
            let e = mode_of(insn, |m| matches!(m, Mode::EInd(r) if r == reg))
                .ok_or_else(|| format!("`{}` has no E,{reg:?} mode", insn.mnemonic))?;
            return Ok(Enc { bytes: e.prefix.to_vec(), unresolved, width: None });
        }
        // Pick Ind8 only for an absolute offset that fits a byte; a forward
        // reference (defined later in source) takes the 16-bit form even when the
        // value would fit a byte, as MASM's first pass commits. The chosen width
        // is recorded in pass 1 and replayed (`forced`) so it never flips on drift.
        let (off, undef) = match expr::eval(base, lc, sym) {
            Ok(v) => (v, false),
            Err(EvalError::Undefined(n)) => {
                unresolved.push(n);
                (0, true)
            }
            Err(EvalError::Syntax(s)) => return Err(s),
        };
        let wide = match forced {
            Some(w) => w,
            None => !(!undef && !needs_wide(base, cur_line, def_line) && (0..=0xFF).contains(&off)),
        };
        let e = if !wide {
            mode_of(insn, |m| matches!(m, Mode::Ind8(r) if r == reg))
                .or_else(|| mode_of(insn, |m| matches!(m, Mode::Ind16(r) if r == reg)))
        } else {
            mode_of(insn, |m| matches!(m, Mode::Ind16(r) if r == reg))
                .or_else(|| mode_of(insn, |m| matches!(m, Mode::Ind8(r) if r == reg)))
        }
        // `jmp`/`jsr` have only a fixed 20-bit indexed form (no 8/16-bit variants).
        .or_else(|| mode_of(insn, |m| matches!(m, Mode::Ind20(r) if r == reg)))
        .ok_or_else(|| format!("`{}` has no indexed-{reg:?} mode", insn.mnemonic))?;
        // Span-dependent only if both widths exist for this register.
        let span_dep = mode_of(insn, |m| matches!(m, Mode::Ind8(r) if r == reg)).is_some()
            && mode_of(insn, |m| matches!(m, Mode::Ind16(r) if r == reg)).is_some();
        let mut bytes = e.prefix.to_vec();
        emit_be(&mut bytes, off, e.operand_len);
        return Ok(Enc { bytes, unresolved, width: span_dep.then_some(wide) });
    }

    // Relative branch. CPU16 displacements are relative to the instruction start
    // + 6 (the 3-word prefetch pipeline), not the next instruction.
    if let Some(e) = mode_of(insn, |m| matches!(m, Mode::Rel8 | Mode::Rel16)) {
        let target = eval_or_zero(raw, lc, sym, &mut unresolved)?;
        let rel = target - (lc as i64 + 6);
        let limit = if e.operand_len == 1 { (-128, 127) } else { (-32768, 32767) };
        if unresolved.is_empty() && (rel < limit.0 || rel > limit.1) {
            return Err(format!("`{}` branch target out of range ({rel})", insn.mnemonic));
        }
        let mut bytes = e.prefix.to_vec();
        emit_be(&mut bytes, rel, e.operand_len);
        return Ok(Enc { bytes, unresolved, width: None });
    }

    // Extended (16- or 20-bit absolute address).
    if let Some(e) = mode_of(insn, |m| matches!(m, Mode::Ext | Mode::Ext20)) {
        let addr = eval_or_zero(raw, lc, sym, &mut unresolved)?;
        let mut bytes = e.prefix.to_vec();
        emit_be(&mut bytes, addr, e.operand_len);
        return Ok(Enc { bytes, unresolved, width: None });
    }

    Err(format!("`{}`: could not encode operand `{raw}`", insn.mnemonic))
}

fn mode_of(insn: &InsnDef, pred: impl Fn(Mode) -> bool) -> Option<&'static ModeEntry> {
    insn.modes.iter().find(|m| pred(m.mode))
}

fn eval_or_zero(expr_s: &str, lc: u32, sym: &SymbolTable, unresolved: &mut Vec<String>) -> Result<i64, String> {
    match expr::eval(expr_s, lc, sym) {
        Ok(v) => Ok(v),
        Err(EvalError::Undefined(n)) => {
            unresolved.push(n);
            Ok(0)
        }
        Err(EvalError::Syntax(s)) => Err(s),
    }
}

fn emit_be(buf: &mut Vec<u8>, value: i64, width: u8) {
    let v = value as u64;
    for shift in (0..width).rev() {
        buf.push((v >> (8 * shift)) as u8);
    }
}

fn is_x_reg(s: &str) -> bool {
    s.trim().eq_ignore_ascii_case("x")
}

fn parse_reg(s: &str) -> Option<IdxReg> {
    match s.trim().to_ascii_lowercase().as_str() {
        "x" => Some(IdxReg::X),
        "y" => Some(IdxReg::Y),
        "z" => Some(IdxReg::Z),
        _ => None,
    }
}

/// Mask bit for a `pshm`/`pulm` register. Push order d,e,x,y,z,k,ccr = bits 0..6;
/// pull uses the bit-reversed assignment.
fn reg_mask_bit(name: &str, pull: bool) -> Option<u8> {
    let idx = match name.to_ascii_lowercase().as_str() {
        "d" => 0,
        "e" => 1,
        "x" => 2,
        "y" => 3,
        "z" => 4,
        "k" => 5,
        "ccr" => 6,
        _ => return None,
    };
    Some(if pull { 1 << (6 - idx) } else { 1 << idx })
}

/// If `raw` ends in `,x` / `,y` / `,z`, return the base and the index register.
fn split_index(raw: &str) -> Option<(&str, IdxReg)> {
    let comma = raw.rfind(',')?;
    let reg = raw[comma + 1..].trim();
    let r = match reg {
        _ if reg.eq_ignore_ascii_case("x") => IdxReg::X,
        _ if reg.eq_ignore_ascii_case("y") => IdxReg::Y,
        _ if reg.eq_ignore_ascii_case("z") => IdxReg::Z,
        _ => return None,
    };
    Some((raw[..comma].trim(), r))
}

/// Split on top-level commas (ignoring those inside single quotes or parens).
fn split_top_commas(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let bytes = s.as_bytes();
    let (mut start, mut depth, mut sq, mut dq) = (0usize, 0i32, false, false);
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'\'' if !dq => sq = !sq,
            b'"' if !sq => dq = !dq,
            b'(' if !sq && !dq => depth += 1,
            b')' if !sq && !dq => depth -= 1,
            b',' if !sq && !dq && depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

// ---- Conditional assembly ---------------------------------------------------

/// One `if … [else] … endc` frame. `emitting` is whether this branch is currently
/// active; `parent_emit` is whether the enclosing context was active.
struct CondFrame {
    parent_emit: bool,
    taken: bool,
    emitting: bool,
}

fn cond_emitting(stack: &[CondFrame]) -> bool {
    stack.last().map_or(true, |f| f.emitting)
}

fn is_if_directive(op: &str) -> bool {
    matches!(
        op,
        "if" | "ifgt" | "iflt" | "ifge" | "ifle" | "ifeq" | "ifne" | "ifdef" | "ifndef" | "ifc" | "ifnc"
    )
}

fn eval_cond(op: &str, operand: Option<&str>, lc: u32, sym: &SymbolTable) -> Option<bool> {
    let num = |e: &str| expr::eval(e, lc, sym).ok();
    match op {
        "ifgt" => operand.and_then(num).map(|v| v > 0),
        "iflt" => operand.and_then(num).map(|v| v < 0),
        "ifge" => operand.and_then(num).map(|v| v >= 0),
        "ifle" => operand.and_then(num).map(|v| v <= 0),
        "ifeq" => operand.and_then(num).map(|v| v == 0),
        "ifne" | "if" => operand.and_then(num).map(|v| v != 0),
        "ifdef" => operand.map(|s| sym.contains(s.trim())),
        "ifndef" => operand.map(|s| !sym.contains(s.trim())),
        "ifc" | "ifnc" => operand.map(|o| {
            let parts = split_top_commas(o);
            let eq = parts.len() == 2 && parts[0].trim() == parts[1].trim();
            if op == "ifc" { eq } else { !eq }
        }),
        _ => None,
    }
}

/// Listing/section/linkage directives that don't yet affect the emitted image.
/// (Section + relocation support will replace the `*sct`/`x*` no-ops.)
const IGNORED_DIRECTIVES: &[&str] = &[
    "mlist", "alist", "clist", "list", "nolist", "nol", "llen", "plen", "page", "nopage",
    "newpage", "title", "ttl", "sttl", "spc", "tabs", "opt", "base", "lll",
    "asct", "bsct", "psct", "dsct", "csct", "idsct", "ipsct", "section",
    "xdef", "xref", "xrefb", "global", "public", "extern", "regdef", "lreg", "file", "name", "idnt",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn asm(src: &str) -> Object {
        assemble_source(src)
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn basic_modes_match_oracle() {
        // Bytes are the authoritative MASM output for this exact source.
        let src = "        org $2000\n\
                   start   ldaa #$12\n\
                   \x20       ldab $40\n\
                   \x20       addd #$1234\n\
                   \x20       jsr start\n\
                   \x20       rts\n\
                   \x20       end\n";
        let obj = asm(src);
        assert!(!obj.has_errors(), "diagnostics: {:?}", obj.diagnostics);
        assert_eq!(hex(&obj.bytes()), "75 12 17 F5 00 40 37 B1 12 34 FA 00 20 00 27 F7");
    }

    #[test]
    fn indexed_and_branches() {
        let src = "        org $2000\n\
                   \x20       ldaa $10,x\n\
                   \x20       ldd $20,y\n\
                   lbl     bra lbl\n\
                   \x20       beq lbl\n\
                   \x20       end\n";
        let obj = asm(src);
        assert!(!obj.has_errors(), "diagnostics: {:?}", obj.diagnostics);
        // ldaa $10,x=45 10 ; ldd $20,y=95 20 ; bra lbl(@2004)=B0 FA ; beq lbl(@2006)=B7 F8
        assert_eq!(hex(&obj.bytes()), "45 10 95 20 B0 FA B7 F8");
    }

    #[test]
    fn directives_emit_bytes() {
        let src = "        org $1000\n\
                   \x20       fcb $12,$34,'A\n\
                   \x20       fdb $1234,$5678\n\
                   \x20       end\n";
        let obj = asm(src);
        assert!(!obj.has_errors(), "diagnostics: {:?}", obj.diagnostics);
        // 7 data bytes is an odd section length, so MASM appends an FF fill byte.
        assert_eq!(hex(&obj.bytes()), "12 34 41 12 34 56 78 FF");
    }

    #[test]
    fn reglist_mov_and_mac() {
        let src = "        org $2000\n\
                   \x20       pshm d,e\n\
                   \x20       pulm d,e\n\
                   \x20       pshm d,e,x,y,z,k,ccr\n\
                   \x20       movb $1000,$2000\n\
                   \x20       movb $08,x,$2000\n\
                   \x20       movb $2000,$08,x\n\
                   \x20       rmac $04,$06\n\
                   \x20       end\n";
        let obj = asm(src);
        assert!(!obj.has_errors(), "diagnostics: {:?}", obj.diagnostics);
        assert_eq!(
            hex(&obj.bytes()),
            "34 03 35 60 34 7F 37 FE 10 00 20 00 30 08 20 00 32 08 20 00 FB 46"
        );
    }

    #[test]
    fn span_dependent_sizing_matches_oracle() {
        // Authoritative MASM bytes (DOSBox oracle) locking in the span-dependent
        // operand-size rules reverse-engineered from the corpus:
        //  - 8-bit immediate is SIGNED: `adde #$7f` -> Imm8, `adde #$80` -> Imm16.
        //  - a `$`-hex offset with letter digits must not be read as a forward
        //    symbol: `ldab $3e,x` stays Ind8 (C5 3E), not Ind16.
        //  - `jsr`/`jmp` indexed use a fixed 20-bit (3-byte) offset.
        //  - an indexed bit-branch sizes by its TARGET: a forward target forces
        //    the 16-bit form (rel16), a backward one keeps the 8-bit form.
        let src = "        org $2000\n\
                   back    rts\n\
                   \x20       adde #$7f\n\
                   \x20       adde #$80\n\
                   \x20       addd #$80\n\
                   \x20       ldab $3e,x\n\
                   \x20       jsr $20000,z\n\
                   \x20       jmp $30000,z\n\
                   \x20       brclr 0,x,#$01,fwd\n\
                   \x20       brclr 0,x,#$01,back\n\
                   fwd     rts\n\
                   \x20       end\n";
        let obj = asm(src);
        assert!(!obj.has_errors(), "diagnostics: {:?}", obj.diagnostics);
        assert_eq!(
            hex(&obj.bytes()),
            "27 F7 7C 7F 37 31 00 80 37 B1 00 80 C5 3E A9 02 00 00 \
             6B 03 00 00 0A 01 00 00 00 04 CB 01 00 DE 27 F7"
        );
    }

    #[test]
    fn macros_and_conditionals() {
        let src = "        org $2000\n\
                   TWO:    macro\n\
                   \x20       ldaa #\\1\n\
                   \x20       ldab #\\2\n\
                   \x20       endm\n\
                   \x20       TWO $11,$22\n\
                   \x20       ifgt 5-3\n\
                   \x20       ldx #$aaaa\n\
                   \x20       elsec\n\
                   \x20       ldx #$bbbb\n\
                   \x20       endc\n\
                   \x20       ifne 0\n\
                   \x20       ldy #$cccc\n\
                   \x20       endc\n\
                   \x20       abx\n\
                   \x20       end\n";
        let obj = asm(src);
        assert!(!obj.has_errors(), "diagnostics: {:?}", obj.diagnostics);
        // TWO -> 75 11 F5 22 ; ifgt true -> 37 BC AA AA ; ifne false skipped ; abx -> 37 4F
        assert_eq!(hex(&obj.bytes()), "75 11 F5 22 37 BC AA AA 37 4F");
    }

    #[test]
    fn include_splices_a_file() {
        let dir = std::env::temp_dir().join("hc16_inc_test");
        std::fs::create_dir_all(&dir).unwrap();
        let inc = dir.join("INC.ASM");
        std::fs::write(&inc, "        ldab #$34\r\n").unwrap();
        let src = "        org $2000\n\
                   \x20       ldaa #$12\n\
                   \x20       include INC.ASM\n\
                   \x20       rts\n\
                   \x20       end\n";
        let obj = assemble_source_in(src, Some(&dir));
        assert!(!obj.has_errors(), "diagnostics: {:?}", obj.diagnostics);
        assert_eq!(hex(&obj.bytes()), "75 12 F5 34 27 F7");
        let _ = std::fs::remove_file(&inc);
    }
}
