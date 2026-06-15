//! Symbol table. MASM symbols are case-sensitive by default (the `-c` option
//! makes them insensitive; the original build does not use it), so we store
//! names verbatim.
//!
//! Each symbol carries a [`Kind`]: a `equ`/`set` to a constant is [`Kind::Abs`]
//! (sized by value), while a label — or any expression involving one — is
//! [`Kind::Rel`] (an address). MASM uses the wide operand form for relocatable
//! offsets even when the value would fit a byte, so tracking this is required
//! for byte-exact output.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Absolute constant.
    Abs,
    /// Relocatable / address-like.
    Rel,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SymbolTable {
    map: HashMap<String, (i64, Kind)>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn define(&mut self, name: &str, value: i64, kind: Kind) {
        self.map.insert(name.to_string(), (value, kind));
    }

    pub fn get(&self, name: &str) -> Option<i64> {
        self.map.get(name).map(|(v, _)| *v)
    }

    pub fn get_full(&self, name: &str) -> Option<(i64, Kind)> {
        self.map.get(name).copied()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, i64)> {
        self.map.iter().map(|(k, (v, _))| (k, *v))
    }
}
