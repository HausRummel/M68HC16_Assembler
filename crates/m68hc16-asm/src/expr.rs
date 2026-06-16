//! Operand expression evaluator.
//!
//! Supports the MASM operand grammar seen in the corpus: `$hex`, `%binary`,
//! `@octal`, decimal, `'c'` character literals, the `*` location counter, symbols,
//! parentheses, the binary operators `+ - * / % & | ^ << >>`, unary `- ~`, and the
//! `PAGE(x)` built-in. A Pratt (precedence-climbing) parser keeps `*` unambiguous:
//! in prefix position it is the location counter, in infix position multiplication.
//!
//! Each result also carries a *relocation count*: the net number of relocatable
//! (address-like) terms. `0` means the expression is absolute. `label + 4` is `1`;
//! `labelA - labelB` is `0` (a constant). MASM uses the wide operand form whenever
//! the count is non-zero, even if the value fits a byte — see [`eval_full`].

use crate::symbols::{Kind, SymbolTable};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    /// A referenced symbol is not (yet) defined.
    Undefined(String),
    /// Malformed expression.
    Syntax(String),
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalError::Undefined(s) => write!(f, "undefined symbol \"{s}\""),
            EvalError::Syntax(s) => write!(f, "{s}"),
        }
    }
}

/// Value paired with its net relocation count.
type V = (i64, i32);

/// Evaluate `input`, returning just the value.
pub fn eval(input: &str, lc: u32, symbols: &SymbolTable) -> Result<i64, EvalError> {
    eval_full(input, lc, symbols).map(|(v, _)| v)
}

/// Evaluate `input`, returning `(value, reloc)`. `reloc == 0` means absolute.
pub fn eval_full(input: &str, lc: u32, symbols: &SymbolTable) -> Result<V, EvalError> {
    let mut p = P { s: input.as_bytes(), i: 0, lc, sym: symbols };
    let v = p.expr(0)?;
    p.ws();
    if p.i != p.s.len() {
        return Err(EvalError::Syntax(format!("unexpected `{}`", &input[p.i..])));
    }
    Ok(v)
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
    lc: u32,
    sym: &'a SymbolTable,
}

impl<'a> P<'a> {
    fn ws(&mut self) {
        while self.i < self.s.len() && (self.s[self.i] == b' ' || self.s[self.i] == b'\t') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn expr(&mut self, min_bp: u8) -> Result<V, EvalError> {
        self.ws();
        let mut lhs = self.prefix()?;
        loop {
            self.ws();
            let Some((op, lbp, rbp)) = self.peek_infix() else { break };
            if lbp < min_bp {
                break;
            }
            self.i += op.len();
            let rhs = self.expr(rbp)?;
            lhs = apply(op, lhs, rhs)?;
        }
        Ok(lhs)
    }

    fn prefix(&mut self) -> Result<V, EvalError> {
        self.ws();
        match self.peek() {
            Some(b'-') => {
                self.i += 1;
                let (v, r) = self.expr(100)?;
                Ok((-v, -r))
            }
            Some(b'~') => {
                self.i += 1;
                let (v, r) = self.expr(100)?;
                Ok((!v, nz(r)))
            }
            Some(b'+') => {
                self.i += 1;
                self.expr(100)
            }
            Some(b'(') => {
                self.i += 1;
                let v = self.expr(0)?;
                self.ws();
                if self.peek() == Some(b')') {
                    self.i += 1;
                    Ok(v)
                } else {
                    Err(EvalError::Syntax("expected `)`".into()))
                }
            }
            Some(b'*') => {
                // Prefix `*` = location counter (an address -> relocatable).
                self.i += 1;
                Ok((self.lc as i64, 1))
            }
            _ => self.atom(),
        }
    }

    fn atom(&mut self) -> Result<V, EvalError> {
        match self.peek() {
            Some(b'$') => {
                self.i += 1;
                Ok((self.radix(16, |c| c.is_ascii_hexdigit())?, 0))
            }
            Some(b'%') => {
                self.i += 1;
                Ok((self.radix(2, |c| c == b'0' || c == b'1')?, 0))
            }
            Some(b'@') => {
                self.i += 1;
                Ok((self.radix(8, |c| (b'0'..=b'7').contains(&c))?, 0))
            }
            // Character constant, single- or double-quoted: MASM accepts both
            // `'A'` and `"A"` in an expression as the char's value.
            Some(q @ (b'\'' | b'"')) => {
                self.i += 1;
                let Some(c) = self.peek() else {
                    return Err(EvalError::Syntax("empty char literal".into()));
                };
                self.i += 1;
                if self.peek() == Some(q) {
                    self.i += 1;
                }
                Ok((c as i64, 0))
            }
            Some(c) if c.is_ascii_digit() => Ok((self.radix(10, |c| c.is_ascii_digit())?, 0)),
            Some(c) if is_sym_start(c) => self.symbol(),
            _ => Err(EvalError::Syntax("expected operand".into())),
        }
    }

    fn radix(&mut self, base: i64, valid: fn(u8) -> bool) -> Result<i64, EvalError> {
        let start = self.i;
        while self.i < self.s.len() && valid(self.s[self.i]) {
            self.i += 1;
        }
        if self.i == start {
            return Err(EvalError::Syntax("expected digits".into()));
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).unwrap();
        i64::from_str_radix(text, base as u32).map_err(|_| EvalError::Syntax(format!("bad number `{text}`")))
    }

    fn symbol(&mut self) -> Result<V, EvalError> {
        let start = self.i;
        while self.i < self.s.len() && is_sym_char(self.s[self.i]) {
            self.i += 1;
        }
        let name = std::str::from_utf8(&self.s[start..self.i]).unwrap();
        // Built-in function call: NAME(expr). The result is an absolute value.
        if self.peek() == Some(b'(') {
            self.i += 1;
            let (arg, _) = self.expr(0)?;
            self.ws();
            if self.peek() != Some(b')') {
                return Err(EvalError::Syntax("expected `)`".into()));
            }
            self.i += 1;
            return Ok((apply_func(name, arg)?, 0));
        }
        match self.sym.get_full(name) {
            Some((v, Kind::Rel)) => Ok((v, 1)),
            Some((v, Kind::Abs)) => Ok((v, 0)),
            None => Err(EvalError::Undefined(name.to_string())),
        }
    }

    fn peek_infix(&self) -> Option<(&'static [u8], u8, u8)> {
        let rest = &self.s[self.i..];
        if rest.starts_with(b"<<") {
            return Some((b"<<", 50, 51));
        }
        if rest.starts_with(b">>") {
            return Some((b">>", 50, 51));
        }
        match rest.first().copied()? {
            b'|' => Some((b"|", 10, 11)),
            b'^' => Some((b"^", 20, 21)),
            b'&' => Some((b"&", 30, 31)),
            b'+' => Some((b"+", 60, 61)),
            b'-' => Some((b"-", 60, 61)),
            b'*' => Some((b"*", 70, 71)),
            b'/' => Some((b"/", 70, 71)),
            b'%' => Some((b"%", 70, 71)),
            _ => None,
        }
    }
}

/// Non-zero-collapse: any non-zero relocation becomes a single relocatable term.
fn nz(r: i32) -> i32 {
    if r != 0 { 1 } else { 0 }
}

fn apply(op: &[u8], (a, ra): V, (b, rb): V) -> Result<V, EvalError> {
    let val = match op {
        b"|" => a | b,
        b"^" => a ^ b,
        b"&" => a & b,
        b"<<" => a << b,
        b">>" => a >> b,
        b"+" => a + b,
        b"-" => a - b,
        b"*" => a * b,
        b"/" => {
            if b == 0 {
                return Err(EvalError::Syntax("division by zero".into()));
            }
            a / b
        }
        b"%" => {
            if b == 0 {
                return Err(EvalError::Syntax("modulo by zero".into()));
            }
            a % b
        }
        _ => unreachable!(),
    };
    // `+`/`-` combine relocation linearly (so label-label cancels); other operators
    // yield a relocatable result if either side was relocatable.
    let reloc = match op {
        b"+" => ra + rb,
        b"-" => ra - rb,
        _ => nz(ra | rb),
    };
    Ok((val, reloc))
}

/// Built-in MASM operand functions. `PAGE(x)` is the bank byte of a 20-bit
/// address (used to set bank registers in the HC16 addressing convention).
fn apply_func(name: &str, arg: i64) -> Result<i64, EvalError> {
    match name.to_ascii_uppercase().as_str() {
        "PAGE" => Ok((arg >> 16) & 0xFF),
        other => Err(EvalError::Undefined(format!("{other}()"))),
    }
}

fn is_sym_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'.'
}

fn is_sym_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'.'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> i64 {
        eval(s, 0, &SymbolTable::new()).unwrap()
    }

    #[test]
    fn numbers_and_radixes() {
        assert_eq!(ev("$1234"), 0x1234);
        assert_eq!(ev("%1010"), 0b1010);
        assert_eq!(ev("@17"), 0o17);
        assert_eq!(ev("42"), 42);
        assert_eq!(ev("'A"), 65);
        assert_eq!(ev("'A'"), 65);
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(ev("1+2*3"), 7);
        assert_eq!(ev("(1+2)*3"), 9);
        assert_eq!(ev("$10+$10"), 0x20);
        assert_eq!(ev("8>>1"), 4);
        assert_eq!(ev("1<<4"), 16);
        assert_eq!(ev("$F0|$0F"), 0xFF);
        assert_eq!(ev("-5"), -5);
        assert_eq!(ev("~0 & $FF"), 0xFF);
    }

    #[test]
    fn star_is_location_counter_in_prefix_and_multiply_in_infix() {
        assert_eq!(eval("*", 0x2000, &SymbolTable::new()).unwrap(), 0x2000);
        assert_eq!(eval("*+4", 0x2000, &SymbolTable::new()).unwrap(), 0x2004);
        assert_eq!(eval("2*3", 0, &SymbolTable::new()).unwrap(), 6);
    }

    #[test]
    fn symbols_resolve_and_report_undefined() {
        let mut t = SymbolTable::new();
        t.define("FOO", 0x40, Kind::Abs);
        assert_eq!(eval("FOO+1", 0, &t).unwrap(), 0x41);
        assert_eq!(eval("BAR", 0, &t), Err(EvalError::Undefined("BAR".into())));
    }

    #[test]
    fn relocation_tracking() {
        let mut t = SymbolTable::new();
        t.define("LBL", 0x90, Kind::Rel);
        t.define("OTHER", 0x10, Kind::Rel);
        t.define("RB", 0x10, Kind::Abs);
        // absolute expressions
        assert_eq!(eval_full("$12", 0, &t).unwrap(), (0x12, 0));
        assert_eq!(eval_full("RB+1", 0, &t).unwrap(), (0x11, 0));
        // relocatable: a label, or label minus an absolute
        assert_eq!(eval_full("LBL", 0, &t).unwrap().1, 1);
        assert_eq!(eval_full("LBL-RB", 0, &t).unwrap(), (0x80, 1));
        // label minus label cancels to absolute
        assert_eq!(eval_full("LBL-OTHER", 0, &t).unwrap(), (0x80, 0));
        // PAGE() is absolute
        assert_eq!(eval_full("PAGE(LBL)", 0, &t).unwrap().1, 0);
    }
}
