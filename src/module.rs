//! Storage for global values used by a takolisp evaluation.
//!
//! A [`Module`] maps symbols to values. Missing symbols return
//! [`Val::Undefined`]; the VM is responsible for turning that value into a
//! runtime error when evaluating a lookup.

use std::{collections::HashMap, path::PathBuf};

use crate::{Val, symbol::Symbol};

/// An index into a module's value storage.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct ModuleSlot(u32);

impl ModuleSlot {
    /// Creates a slot referring to `index`.
    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    /// Returns the underlying value storage index.
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// A collection of global values and the source that owns them.
#[derive(Clone, Debug)]
pub struct Module {
    pub source: ModuleSource,
    slots: HashMap<Symbol, ModuleSlot>,
    values: Vec<Val>,
}

/// The source associated with a module.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ModuleSource {
    Global,
    Internal(&'static str),
    File(PathBuf),
}

impl Module {
    pub fn new(source: ModuleSource) -> Self {
        Self {
            source,
            slots: HashMap::new(),
            values: Vec::new(),
        }
    }

    /// Returns the value associated with `symbol`, or [`Val::Undefined`] when
    /// the symbol has not been defined.
    pub fn get(&self, symbol: Symbol) -> Val {
        self.slots
            .get(&symbol)
            .map(|&slot| self.get_slot(slot))
            .unwrap_or_default()
    }

    /// Returns the slot associated with `symbol`, if it has been defined.
    pub fn slot(&self, symbol: Symbol) -> Option<ModuleSlot> {
        self.slots.get(&symbol).copied()
    }

    /// Returns the symbol associated with `slot`, if it is valid.
    pub fn symbol(&self, slot: ModuleSlot) -> Option<Symbol> {
        self.slots
            .iter()
            .find_map(|(&symbol, &candidate)| (candidate == slot).then_some(symbol))
    }

    /// Defines `symbol` and returns its stable slot.
    pub fn define(&mut self, symbol: Symbol) -> ModuleSlot {
        if let Some(&slot) = self.slots.get(&symbol) {
            return slot;
        }

        let slot = ModuleSlot(self.values.len() as u32);
        self.slots.insert(symbol, slot);
        self.values.push(Val::Undefined);
        slot
    }

    /// Returns the value stored in `slot`, or [`Val::Undefined`] for an invalid slot.
    pub fn get_slot(&self, slot: ModuleSlot) -> Val {
        self.values
            .get(slot.0 as usize)
            .copied()
            .unwrap_or_default()
    }

    /// Sets the value stored in `slot`.
    ///
    /// Returns `false` without storing `val` if `slot` was not [`Self::define`]d
    /// by this module.
    pub fn set_slot(&mut self, slot: ModuleSlot, val: Val) -> bool {
        match self.values.get_mut(slot.0 as usize) {
            Some(entry) => {
                *entry = val;
                true
            }
            None => false,
        }
    }

    /// Sets or replaces the value associated with `symbol`.
    pub fn set(&mut self, symbol: Symbol, val: Val) {
        match self.slots.get(&symbol) {
            Some(slot) => self.values[slot.index() as usize] = val,
            None => {
                let slot = ModuleSlot(self.values.len() as u32);
                self.slots.insert(symbol, slot);
                self.values.push(val);
            }
        }
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = Val> + '_ {
        self.values.iter().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_undefined_for_missing_values() {
        let module = Module::new(ModuleSource::Internal("test"));

        assert_eq!(module.get(Symbol::must_inline("missing")), Val::Undefined);
    }

    #[test]
    fn stores_and_retrieves_values() {
        let mut module = Module::new(ModuleSource::Internal("test"));
        let symbol = Symbol::must_inline("answer");

        module.set(symbol, Val::Int(42));

        assert_eq!(module.get(symbol), Val::Int(42));
    }

    #[test]
    fn replaces_existing_values() {
        let mut module = Module::new(ModuleSource::Internal("test"));
        let symbol = Symbol::must_inline("answer");

        module.set(symbol, Val::Int(41));
        module.set(symbol, Val::Int(42));

        assert_eq!(module.get(symbol), Val::Int(42));
    }

    #[test]
    fn set_undefined_slot_is_rejected() {
        let mut module = Module::new(ModuleSource::Internal("test"));
        let symbol = Symbol::must_inline("answer");
        let slot = module.define(symbol);

        assert!(module.set_slot(slot, Val::Int(42)));
        assert!(!module.set_slot(ModuleSlot::new(slot.index() + 1), Val::Int(43)));

        assert_eq!(module.get(symbol), Val::Int(42));
    }
}
