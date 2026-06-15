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

/// Assemble a source string, resolving `include` files relative to `base_dir`.
/// Includes and macros are expanded once up front; the result is then run through
/// the fixpoint passes (which evaluate conditionals using the live location
/// counter and symbols).
pub fn assemble_source_in(src: &str, base_dir: Option<&Path>) -> Object {
    let raw: Vec<String> = src.lines().map(str::to_string).collect();
    let lines = match preprocess(&raw, base_dir) {
        Ok(l) => l,
        Err(diags) => {
            return Object { diagnostics: diags, ..Object::default() };
        }
    };
    // First definition line of each symbol (1-based, matching run_pass `lineno`).
    // Used to size operands: a reference to a symbol defined *later* (forward
    // reference) commits to the wide form, as MASM's first pass does.
    let def_line = definition_lines(&lines);

    let mut symbols = SymbolTable::new();
    let mut prev: Option<Vec<(u32, u8)>> = None;
    for _ in 0..40 {
        let obj = run_pass(&lines, &symbols, &def_line);
        if prev.as_ref() == Some(&obj.data) {
            if let Ok(path) = std::env::var("HC16_SYMS") {
                use std::fmt::Write as _;
                let mut s = String::new();
                for (k, v) in obj.symbols.iter() {
                    let _ = writeln!(s, "{k}\t{v:X}");
                }
                let _ = std::fs::write(&path, s);
            }
            return obj;
        }
        prev = Some(obj.data.clone());
        symbols = obj.symbols.clone();
    }
    let mut obj = run_pass(&lines, &symbols, &def_line);
    obj.diagnostics
        .push(Diagnostic::warning("assembly did not converge (possible phase error)"));
    obj
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
fn needs_wide(expr: &str, cur_line: u32, def_line: &HashMap<String, usize>) -> bool {
    let b = expr.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_alphabetic() || b[i] == b'_' || b[i] == b'.' {
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
        } else {
            i += 1;
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

fn run_pass(lines: &[String], sym_in: &SymbolTable, def_line: &HashMap<String, usize>) -> Object {
    let mut out = Object::default();
    let mut lc: u32 = 0;
    // Section origin (first `org`); used to pad the section to an even length, which
    // MASM does automatically ("section has an odd length; fill byte was added").
    let mut origin: Option<u32> = None;
    let mut cond_stack: Vec<CondFrame> = Vec::new();

    for (idx, raw_line) in lines.iter().enumerate() {
        let lineno = (idx + 1) as u32;
        let line = split_line(raw_line);
        let op_lower = line.op.map(|o| o.to_ascii_lowercase());

        // Conditional assembly: keep the stack balanced even in skipped regions.
        if let Some(op) = op_lower.as_deref() {
            if is_if_directive(op) {
                let pe = cond_emitting(&cond_stack);
                let cond = pe && eval_cond(op, line.operand, lc, sym_in).unwrap_or(false);
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
                        match expr::eval_full(first, lc, sym_in) {
                            Ok((v, r)) => {
                                out.symbols.define(lbl, v, if r == 0 { Kind::Abs } else { Kind::Rel })
                            }
                            Err(EvalError::Undefined(_)) => {} // resolves on a later pass
                            Err(EvalError::Syntax(s)) => err(&mut out, lineno, s),
                        }
                    } else {
                        err(&mut out, lineno, "equ requires an operand".into());
                    }
                }
                // A label is an address -> relocatable.
                _ => out.symbols.define(lbl, lc as i64, Kind::Rel),
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
            "org" => match eval1(line.operand, lc, sym_in) {
                Ok(v) => {
                    lc = v as u32;
                    origin.get_or_insert(lc);
                }
                Err(e) => err(&mut out, lineno, format!("org: {e}")),
            },
            "rmb" | "ds" => match eval1(line.operand, lc, sym_in) {
                Ok(v) => lc = lc.wrapping_add(v as u32),
                Err(e) => err(&mut out, lineno, format!("rmb: {e}")),
            },
            // Alignment: MASM emits 0xFF fill bytes up to the boundary.
            "even" => {
                if lc & 1 != 0 {
                    out.data.push((lc, 0xFF));
                    lc = lc.wrapping_add(1);
                }
            }
            "longeven" => {
                while lc & 3 != 0 {
                    out.data.push((lc, 0xFF));
                    lc = lc.wrapping_add(1);
                }
            }
            "fcb" | "dc.b" => emit_list(&mut out, &mut lc, lineno, line.operand, sym_in, 1),
            "fdb" | "dc.w" => emit_list(&mut out, &mut lc, lineno, line.operand, sym_in, 2),
            "fcc" => emit_fcc(&mut out, &mut lc, lineno, line.operand),
            "end" => break,
            _ => match isa::lookup(op) {
                Some(insn) => match encode_instruction(insn, line.operand, lc, sym_in, lineno, def_line) {
                    Ok(enc) => {
                        for n in enc.unresolved {
                            err(&mut out, lineno, format!("undefined symbol \"{n}\""));
                        }
                        for b in enc.bytes {
                            out.data.push((lc, b));
                            lc = lc.wrapping_add(1);
                        }
                    }
                    Err(msg) => err(&mut out, lineno, msg),
                },
                None => err(&mut out, lineno, format!("unknown operation `{}`", line.op.unwrap())),
            },
        }
    }

    // Pad the section to an even length (MASM appends a 0xFF fill byte).
    if let Some(o) = origin {
        if lc.wrapping_sub(o) & 1 == 1 {
            out.data.push((lc, 0xFF));
        }
    }

    out.symbols = merge(sym_in, &out.symbols);
    out
}

/// Carry over previously-known symbols so multi-pass resolution accumulates.
fn merge(base: &SymbolTable, new: &SymbolTable) -> SymbolTable {
    let _ = base;
    new.clone()
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
}

fn encode_instruction(
    insn: &InsnDef,
    operand: Option<&str>,
    lc: u32,
    sym: &SymbolTable,
    cur_line: u32,
    def_line: &HashMap<String, usize>,
) -> Result<Enc, String> {
    let mut unresolved = Vec::new();

    // Inherent-only instructions ignore any operand — MASM does too (the operand
    // column often just holds a trailing comment the lexer captured).
    if insn.modes.iter().all(|m| matches!(m.mode, Mode::Inherent)) {
        if let Some(e) = mode_of(insn, |m| matches!(m, Mode::Inherent)) {
            return Ok(Enc { bytes: e.prefix.to_vec(), unresolved });
        }
    }

    // No operand -> inherent.
    let Some(raw) = operand.map(str::trim).filter(|s| !s.is_empty()) else {
        let e = mode_of(insn, |m| matches!(m, Mode::Inherent))
            .ok_or_else(|| format!("`{}` requires an operand", insn.mnemonic))?;
        return Ok(Enc { bytes: e.prefix.to_vec(), unresolved });
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
        let fits8 = !undef && !needs_wide(rest, cur_line, def_line) && (0..=0xFF).contains(&v);
        let e = if fits8 { imm8.or(imm16) } else { imm16.or(imm8) }
            .ok_or_else(|| format!("`{}` has no immediate mode", insn.mnemonic))?;
        let mut bytes = e.prefix.to_vec();
        emit_be(&mut bytes, v, e.operand_len);
        return Ok(Enc { bytes, unresolved });
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
                return Ok(Enc { bytes, unresolved });
            }
            [off, reg, mask, tgt] if parse_reg(reg).is_some() => {
                let r = parse_reg(reg).unwrap();
                let o = eval_or_zero(off, lc, sym, &mut unresolved)?;
                let m = eval_or_zero(mask.trim_start_matches('#'), lc, sym, &mut unresolved)?;
                let rel = eval_or_zero(tgt, lc, sym, &mut unresolved)? - (lc as i64 + 6);
                let wide = needs_wide(off, cur_line, def_line) || !(0..=0xFF).contains(&o);
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
                return Ok(Enc { bytes, unresolved });
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
                return Ok(Enc { bytes, unresolved });
            }
            [off, reg, mask] if parse_reg(reg).is_some() => {
                let r = parse_reg(reg).unwrap();
                let o = eval_or_zero(off, lc, sym, &mut unresolved)?;
                let m = eval_or_zero(mask.trim_start_matches('#'), lc, sym, &mut unresolved)?;
                let wide = needs_wide(off, cur_line, def_line) || !(0..=0xFF).contains(&o);
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
                return Ok(Enc { bytes, unresolved });
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
        return Ok(Enc { bytes, unresolved });
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
                return Ok(Enc { bytes, unresolved });
            }
            [off, reg, dst] if is_x_reg(reg) => {
                let e = mode_of(insn, |m| matches!(m, Mode::MovIdxExt))
                    .ok_or_else(|| format!("`{}`: no indexed-source move", insn.mnemonic))?;
                let o = eval_or_zero(off, lc, sym, &mut unresolved)?;
                let d = eval_or_zero(dst, lc, sym, &mut unresolved)?;
                let mut bytes = e.prefix.to_vec();
                emit_be(&mut bytes, o, 1);
                emit_be(&mut bytes, d, 2);
                return Ok(Enc { bytes, unresolved });
            }
            [src, off, reg] if is_x_reg(reg) => {
                let e = mode_of(insn, |m| matches!(m, Mode::MovExtIdx))
                    .ok_or_else(|| format!("`{}`: no indexed-dest move", insn.mnemonic))?;
                let s = eval_or_zero(src, lc, sym, &mut unresolved)?;
                let o = eval_or_zero(off, lc, sym, &mut unresolved)?;
                let mut bytes = e.prefix.to_vec();
                emit_be(&mut bytes, o, 1);
                emit_be(&mut bytes, s, 2);
                return Ok(Enc { bytes, unresolved });
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
        return Ok(Enc { bytes, unresolved });
    }

    // Indexed or accumulator-E indexed: `<base>,<reg>`.
    if let Some((base, reg)) = split_index(raw) {
        if base.eq_ignore_ascii_case("e") {
            let e = mode_of(insn, |m| matches!(m, Mode::EInd(r) if r == reg))
                .ok_or_else(|| format!("`{}` has no E,{reg:?} mode", insn.mnemonic))?;
            return Ok(Enc { bytes: e.prefix.to_vec(), unresolved });
        }
        // Pick Ind8 only for an absolute offset that fits a byte; relocatable
        // (address-like) offsets take the 16-bit form even when small, as MASM does.
        let (off, undef) = match expr::eval(base, lc, sym) {
            Ok(v) => (v, false),
            Err(EvalError::Undefined(n)) => {
                unresolved.push(n);
                (0, true)
            }
            Err(EvalError::Syntax(s)) => return Err(s),
        };
        let fits8 = !undef && !needs_wide(base, cur_line, def_line) && (0..=0xFF).contains(&off);
        let e = if fits8 {
            mode_of(insn, |m| matches!(m, Mode::Ind8(r) if r == reg))
                .or_else(|| mode_of(insn, |m| matches!(m, Mode::Ind16(r) if r == reg)))
        } else {
            mode_of(insn, |m| matches!(m, Mode::Ind16(r) if r == reg))
                .or_else(|| mode_of(insn, |m| matches!(m, Mode::Ind8(r) if r == reg)))
        }
        .ok_or_else(|| format!("`{}` has no indexed-{reg:?} mode", insn.mnemonic))?;
        let mut bytes = e.prefix.to_vec();
        emit_be(&mut bytes, off, e.operand_len);
        return Ok(Enc { bytes, unresolved });
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
        return Ok(Enc { bytes, unresolved });
    }

    // Extended (16- or 20-bit absolute address).
    if let Some(e) = mode_of(insn, |m| matches!(m, Mode::Ext | Mode::Ext20)) {
        let addr = eval_or_zero(raw, lc, sym, &mut unresolved)?;
        let mut bytes = e.prefix.to_vec();
        emit_be(&mut bytes, addr, e.operand_len);
        return Ok(Enc { bytes, unresolved });
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
