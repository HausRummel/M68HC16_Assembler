//! Operand expression evaluator.
//!
//! Supports the MASM operand grammar seen in the corpus: `$hex`, `%binary`,
//! `@octal`, decimal, `'c'` character literals, the `*` location counter, symbols,
//! parentheses, the binary operators `+ - * / % & | ^ << >>`, and unary `- ~`.
//! A Pratt (precedence-climbing) parser keeps `*` unambiguous: in prefix position
//! it is the location counter, in infix position it is multiplication.

use crate::symbols::SymbolTable;

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

/// Evaluate `input` with the current location counter `lc` and `symbols`.
pub fn eval(input: &str, lc: u32, symbols: &SymbolTable) -> Result<i64, EvalError> {
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

    /// Precedence-climbing expression parse. `min_bp` is the minimum binding power.
    fn expr(&mut self, min_bp: u8) -> Result<i64, EvalError> {
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

    fn prefix(&mut self) -> Result<i64, EvalError> {
        self.ws();
        match self.peek() {
            Some(b'-') => {
                self.i += 1;
                Ok(-self.expr(100)?)
            }
            Some(b'~') => {
                self.i += 1;
                Ok(!self.expr(100)?)
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
                // Prefix `*` = location counter.
                self.i += 1;
                Ok(self.lc as i64)
            }
            _ => self.atom(),
        }
    }

    fn atom(&mut self) -> Result<i64, EvalError> {
        match self.peek() {
            Some(b'$') => {
                self.i += 1;
                self.radix(16, |c| c.is_ascii_hexdigit())
            }
            Some(b'%') => {
                self.i += 1;
                self.radix(2, |c| c == b'0' || c == b'1')
            }
            Some(b'@') => {
                self.i += 1;
                self.radix(8, |c| (b'0'..=b'7').contains(&c))
            }
            Some(b'\'') => {
                // Character literal: 'c (closing quote optional, as in Motorola asm).
                self.i += 1;
                let Some(c) = self.peek() else {
                    return Err(EvalError::Syntax("empty char literal".into()));
                };
                self.i += 1;
                if self.peek() == Some(b'\'') {
                    self.i += 1;
                }
                Ok(c as i64)
            }
            Some(c) if c.is_ascii_digit() => self.radix(10, |c| c.is_ascii_digit()),
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
        i64::from_str_radix(text, base as u32)
            .map_err(|_| EvalError::Syntax(format!("bad number `{text}`")))
    }

    fn symbol(&mut self) -> Result<i64, EvalError> {
        let start = self.i;
        while self.i < self.s.len() && is_sym_char(self.s[self.i]) {
            self.i += 1;
        }
        let name = std::str::from_utf8(&self.s[start..self.i]).unwrap();
        // Built-in function call: NAME(expr).
        if self.peek() == Some(b'(') {
            self.i += 1;
            let arg = self.expr(0)?;
            self.ws();
            if self.peek() != Some(b')') {
                return Err(EvalError::Syntax("expected `)`".into()));
            }
            self.i += 1;
            return apply_func(name, arg);
        }
        self.sym
            .get(name)
            .ok_or_else(|| EvalError::Undefined(name.to_string()))
    }

    /// Returns (operator-bytes, left-bp, right-bp) for the infix operator at `i`.
    fn peek_infix(&self) -> Option<(&'static [u8], u8, u8)> {
        let rest = &self.s[self.i..];
        // Multi-byte operators first.
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

fn apply(op: &[u8], a: i64, b: i64) -> Result<i64, EvalError> {
    Ok(match op {
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
    })
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
        t.define("FOO", 0x40);
        assert_eq!(eval("FOO+1", 0, &t).unwrap(), 0x41);
        assert_eq!(eval("BAR", 0, &t), Err(EvalError::Undefined("BAR".into())));
    }
}
