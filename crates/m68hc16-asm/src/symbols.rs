//! Symbol table. MASM symbols are case-sensitive by default (the `-c` option
//! makes them insensitive; the original build does not use it), so we store
//! names verbatim.

use std::collections::HashMap;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SymbolTable {
    map: HashMap<String, i64>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Define or redefine a symbol.
    pub fn define(&mut self, name: &str, value: i64) {
        self.map.insert(name.to_string(), value);
    }

    pub fn get(&self, name: &str) -> Option<i64> {
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
}
