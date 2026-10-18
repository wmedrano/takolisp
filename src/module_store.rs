//! Modules indexed by source and accessible through stable heap handles.

use std::collections::HashMap;

use crate::{
    heap::{HeapHandle, HeapTable},
    module::{Module, ModuleSource},
};

/// Stores modules by their source while preserving stable heap handles.
#[derive(Debug, Default)]
pub struct ModuleStore {
    modules: HeapTable<Module>,
    handles: HashMap<ModuleSource, HeapHandle<Module>>,
}

/// An error returned when a module source is already present in a store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DuplicateModuleSource {
    pub source: ModuleSource,
    pub existing: HeapHandle<Module>,
}

impl std::fmt::Display for DuplicateModuleSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "module source is already registered: {:?}", self.source)
    }
}

impl std::error::Error for DuplicateModuleSource {}

impl ModuleStore {
    /// Insert a module unless its source is already registered.
    /// On a duplicate, the error contains the handle of the existing module.
    pub fn insert(
        &mut self,
        source: ModuleSource,
    ) -> Result<HeapHandle<Module>, DuplicateModuleSource> {
        if let Some(&existing) = self.handles.get(&source) {
            return Err(DuplicateModuleSource { source, existing });
        }
        let handle = self.modules.insert(Module::new(source.clone()));
        self.handles.insert(source, handle);
        Ok(handle)
    }

    /// Remove a module by its stable heap handle.
    pub(crate) fn remove(&mut self, handle: HeapHandle<Module>) {
        let Some(module) = self.modules.remove(handle) else {
            return;
        };
        if self.handles.get(&module.source) == Some(&handle) {
            self.handles.remove(&module.source);
        }
    }

    /// Get a module by its stable heap handle.
    pub fn get(&self, handle: HeapHandle<Module>) -> Option<&Module> {
        self.modules.get(handle)
    }

    /// Get a mutable module by its stable heap handle.
    pub fn get_mut(&mut self, handle: HeapHandle<Module>) -> Option<&mut Module> {
        self.modules.get_mut(handle)
    }

    /// Iterate over registered module handles.
    pub(crate) fn handles(&self) -> impl Iterator<Item = HeapHandle<Module>> + '_ {
        self.modules.iter().map(|(handle, _)| handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_duplicate_sources() {
        let source = ModuleSource::Internal("test");
        let mut modules = ModuleStore::default();
        let handle = modules.insert(source.clone()).unwrap();

        assert_eq!(modules.get(handle).unwrap().source, source);
        assert_eq!(
            modules.insert(source.clone()).unwrap_err(),
            DuplicateModuleSource {
                source,
                existing: handle
            }
        );
    }
}
