//! Heap allocation and storage for takolisp values.
//!
//! [`Heap`] owns the tables used by the runtime, while [`HeapHandle`] provides
//! lightweight references to values stored in a [`HeapTable`].

use std::marker::PhantomData;

use crate::boxed_value::BoxedValue;
use crate::function::{BytecodeFunction, ClosureCaptures};
use crate::native_function::NativeClosure;
use crate::pair::Pair;
use crate::symbol::SymbolTable;

pub use crate::module_store::{DuplicateModuleSource, ModuleStore};

/// Contains all heap allocated objects for a `Vm`.
#[derive(Default, Debug)]
pub struct Heap {
    /// Creates and stores modules.
    pub modules: ModuleStore,
    /// Creates and stores symbols.
    pub symbols: SymbolTable,
    /// Creates and stores strings.
    pub strings: HeapTable<String>,
    /// Creates and stores pairs.
    pub pairs: HeapTable<Pair>,
    /// Creates and stores native closures.
    pub native_closures: HeapTable<NativeClosure>,
    /// Creates and stores bytecode functions.
    pub bytecode_functions: HeapTable<BytecodeFunction>,
    /// Creates and stores boxed values.
    pub boxed_values: HeapTable<BoxedValue>,
    /// Creates and stores closure values.
    pub closures: HeapTable<ClosureCaptures>,
}

/// Contains a handle to a heap allocated object.
pub struct HeapHandle<T> {
    generation: Generation,
    idx: u32,
    _marker: PhantomData<fn() -> T>,
}

/// Stores heap-allocated objects of type `T`.
#[derive(Debug)]
pub struct HeapTable<T> {
    entries: Vec<HeapEntry<T>>,
    free: Option<usize>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(transparent)]
struct Generation(u32);

impl Generation {
    fn increment(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }
}

#[derive(Debug)]
struct HeapEntry<T> {
    generation: Generation,
    kind: HeapEntryKind<T>,
}

#[derive(Debug)]
enum HeapEntryKind<T> {
    Occupied(T),
    Vacant(Option<usize>),
}

impl<T> Default for HeapTable<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            free: None,
        }
    }
}

impl<T> HeapTable<T> {
    /// Insert `value` into the table and return a handle to it.
    pub fn insert(&mut self, value: impl Into<T>) -> HeapHandle<T> {
        if let Some(free_idx) = self.free {
            let next = match &self.entries[free_idx].kind {
                HeapEntryKind::Vacant(next) => *next,
                HeapEntryKind::Occupied(_) => unreachable!("freelist points to an occupied entry"),
            };
            self.entries[free_idx].kind = HeapEntryKind::Occupied(value.into());
            let generation = self.entries[free_idx].generation;
            self.free = next;
            return HeapHandle {
                generation,
                idx: free_idx as u32,
                _marker: PhantomData,
            };
        }

        let (generation, idx) = (Generation(0), self.entries.len() as u32);
        self.entries.push(HeapEntry {
            generation,
            kind: HeapEntryKind::Occupied(value.into()),
        });
        HeapHandle {
            generation,
            idx,
            _marker: PhantomData,
        }
    }

    /// Remove the item pointed to by `handle` and return it.
    ///
    /// Returns `None` if `handle` is invalid or points to an item that was
    /// already removed.
    pub fn remove(&mut self, handle: HeapHandle<T>) -> Option<T> {
        // Remove the previous entry
        let idx = handle.idx as usize;
        let Some(entry) = self.entries.get_mut(idx) else {
            std::hint::cold_path();
            return None;
        };
        if entry.generation != handle.generation {
            std::hint::cold_path();
            return None;
        }
        let previous = std::mem::replace(&mut entry.kind, HeapEntryKind::Vacant(self.free));
        entry.generation.increment();

        // Fix the free list
        match previous {
            HeapEntryKind::Occupied(value) => {
                self.free = Some(idx);
                Some(value)
            }
            HeapEntryKind::Vacant(next) => {
                std::hint::cold_path();
                // This path occurs when we try to double free. We just put the Vacant slot as it
                // was before.
                entry.kind = HeapEntryKind::Vacant(next);
                None
            }
        }
    }

    /// Iterate over occupied entries and their handles.
    pub fn iter(&self) -> impl Iterator<Item = (HeapHandle<T>, &T)> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| match &entry.kind {
                HeapEntryKind::Occupied(value) => Some((
                    HeapHandle {
                        generation: entry.generation,
                        idx: idx as u32,
                        _marker: PhantomData,
                    },
                    value,
                )),
                HeapEntryKind::Vacant(_) => None,
            })
    }

    /// Retain the entries whose handles satisfy `predicate`.
    pub fn retain(&mut self, mut predicate: impl FnMut(HeapHandle<T>) -> bool) {
        for idx in 0..self.entries.len() {
            let entry = &self.entries[idx];
            let handle = HeapHandle {
                generation: entry.generation,
                idx: idx as u32,
                _marker: PhantomData,
            };
            if matches!(entry.kind, HeapEntryKind::Occupied(_)) && !predicate(handle) {
                // TODO(Optimize): We have a chance to both batch removals and avoid calling
                // `remove`. `remove` has redundant checks and assertions.
                self.remove(handle);
            }
        }
    }

    /// Get a reference to the item pointed by `handle`.
    ///
    /// Returns `None` if `handle` is invalid or points to a vacant entry.
    pub fn get(&self, handle: HeapHandle<T>) -> Option<&T> {
        let entry = self.entries.get(handle.idx as usize)?;
        if let HeapEntry {
            generation,
            kind: HeapEntryKind::Occupied(value),
        } = entry
            && *generation == handle.generation
        {
            Some(value)
        } else {
            std::hint::cold_path();
            None
        }
    }

    /// Get a mutable reference to the item pointed by `handle`.
    ///
    /// Returns `None` if `handle` is invalid or points to a vacant entry.
    pub fn get_mut(&mut self, handle: HeapHandle<T>) -> Option<&mut T> {
        let entry = self.entries.get_mut(handle.idx as usize)?;
        if let HeapEntry {
            generation,
            kind: HeapEntryKind::Occupied(value),
        } = entry
            && *generation == handle.generation
        {
            Some(value)
        } else {
            std::hint::cold_path();
            None
        }
    }
}

impl<T> Copy for HeapHandle<T> {}

impl<T> Clone for HeapHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> PartialEq for HeapHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.idx == other.idx
    }
}

impl<T> Eq for HeapHandle<T> {}

impl<T> std::hash::Hash for HeapHandle<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.idx.hash(state);
    }
}

impl<T> HeapHandle<T> {
    pub(crate) const fn index(self) -> u32 {
        self.idx
    }
}

impl<T> std::fmt::Debug for HeapHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("HeapId").field(&self.idx).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Val, function::CaptureStorage, module::ModuleSource};

    #[test]
    fn table_ids_are_copy() {
        let mut table = HeapTable::<String>::default();
        let id: HeapHandle<String> = table.insert("copy");
        let copy = id;
        assert_eq!(id, copy);
        assert_eq!(table.get(copy).unwrap(), "copy");
    }

    #[test]
    fn table_stores_and_retrieves_with_mutation() {
        let mut table = HeapTable::<String>::default();
        let id = table.insert("hello");
        assert_eq!(table.get(id).unwrap(), "hello");
        table.get_mut(id).unwrap().push_str(" world");
        assert_eq!(table.get(id).unwrap(), "hello world");
    }

    #[test]
    fn table_alloc_does_not_deduplicate() {
        let mut table = HeapTable::<String>::default();
        let a = table.insert("dup");
        let b = table.insert("dup");
        assert_ne!(a, b);
        assert_eq!(table.get(a).unwrap(), table.get(b).unwrap());
    }

    #[test]
    fn table_ids_remain_stable_after_further_allocs() {
        let mut table = HeapTable::<String>::default();
        let first = table.insert("first");
        table.insert("second");
        table.insert("third");
        assert_eq!(table.get(first).unwrap(), "first");
    }

    #[test]
    fn table_mutations_are_isolated() {
        let mut table = HeapTable::<String>::default();
        let a = table.insert("a");
        let b = table.insert("b");
        table.get_mut(a).unwrap().push_str("-mut");
        assert_eq!(table.get(a).unwrap(), "a-mut");
        assert_eq!(table.get(b).unwrap(), "b");
    }

    #[test]
    fn table_remove_returns_value_and_reuses_slot() {
        let mut table = HeapTable::<String>::default();
        let first = table.insert("first");
        let second = table.insert("second");

        assert_eq!(table.remove(first), Some("first".to_string()));
        let replacement = table.insert("replacement");

        assert_eq!(replacement, first);
        assert_eq!(table.get(replacement).unwrap(), "replacement");
        assert_eq!(table.get(second).unwrap(), "second");
    }

    #[test]
    fn table_remove_returns_none_for_invalid_or_removed_handles() {
        let mut table = HeapTable::<String>::default();
        let handle = table.insert("value");

        assert_eq!(table.remove(handle), Some("value".to_string()));
        assert_eq!(table.remove(handle), None);
        assert_eq!(
            table.remove(HeapHandle {
                generation: Generation(0),
                idx: 1,
                _marker: PhantomData
            }),
            None
        );
    }

    #[test]
    fn table_get_returns_none_for_invalid_or_removed_handles() {
        let mut table = HeapTable::<String>::default();
        let handle = table.insert("value");

        assert_eq!(table.get(handle), Some(&"value".to_string()));
        assert!(table.get_mut(handle).is_some());

        table.remove(handle);
        assert_eq!(table.get(handle), None);
        assert_eq!(table.get_mut(handle), None);

        let invalid = HeapHandle {
            generation: Generation(0),
            idx: 1,
            _marker: PhantomData,
        };
        assert_eq!(table.get(invalid), None);
        assert_eq!(table.get_mut(invalid), None);
    }

    #[test]
    fn table_reuses_removed_slots_in_lifo_order() {
        let mut table = HeapTable::<String>::default();
        let first = table.insert("first");
        let second = table.insert("second");

        table.remove(first);
        table.remove(second);

        assert_eq!(table.insert("new second"), second);
        assert_eq!(table.insert("new first"), first);
    }

    #[test]
    fn boxed_values_can_be_accessed_and_mutated() {
        let mut table = HeapTable::<BoxedValue>::default();
        let handle = table.insert(BoxedValue::from_value(Val::Int(1)));

        assert_eq!(table.get(handle).unwrap().value(), Val::Int(1));
        *table.get_mut(handle).unwrap().value_mut() = Val::Int(2);
        assert_eq!(table.get(handle).unwrap().value(), Val::Int(2));
    }

    #[test]
    fn closure_values_can_describe_stack_and_boxed_storage() {
        let mut heap = Heap::default();
        let boxed = heap
            .boxed_values
            .insert(BoxedValue::from_value(Val::Int(42)));
        let module = heap.modules.insert(ModuleSource::Internal("test")).unwrap();
        let function = heap
            .bytecode_functions
            .insert(BytecodeFunction::new(module));
        let closure = heap.closures.insert(ClosureCaptures::from_captures(
            function,
            vec![
                CaptureStorage::Stack(3),
                CaptureStorage::Boxed(boxed),
                CaptureStorage::ReadOnlyBoxed(boxed),
            ]
            .into_boxed_slice(),
        ));

        assert_eq!(heap.closures.get(closure).unwrap().values.len(), 3);
        assert_eq!(
            heap.closures.get(closure).unwrap().values[0],
            CaptureStorage::Stack(3)
        );
        assert_eq!(
            heap.closures.get(closure).unwrap().values[1],
            CaptureStorage::Boxed(boxed)
        );
        assert_eq!(
            heap.closures.get(closure).unwrap().values[2],
            CaptureStorage::ReadOnlyBoxed(boxed)
        );
    }
}
