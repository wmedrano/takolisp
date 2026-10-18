//! Mark-and-sweep garbage collection for heap-allocated Lisp values.

use std::collections::HashSet;

use crate::{
    boxed_value::BoxedValue,
    function::{BytecodeFunction, CaptureStorage, ClosureCaptures, Instruction},
    heap::{Heap, HeapHandle},
    module::Module,
    pair::Pair,
    val::Val,
};

#[cfg(test)]
use crate::module::ModuleSource;

/// Marks reachable heap values and removes the rest during finalization.
#[derive(Default)]
pub(crate) struct MarkAndSweepGarbageCollector {
    modules: HashSet<u32>,
    strings: HashSet<u32>,
    pairs: HashSet<u32>,
    native_closures: HashSet<u32>,
    bytecode_functions: HashSet<u32>,
    boxed_values: HashSet<u32>,
    closures: HashSet<u32>,
}

impl MarkAndSweepGarbageCollector {
    /// Clear all marks so the collector can be reused for another collection.
    pub(crate) fn clear(&mut self) {
        self.modules.clear();
        self.strings.clear();
        self.pairs.clear();
        self.bytecode_functions.clear();
        self.boxed_values.clear();
        self.closures.clear();
    }

    /// Mark all values in `vals` and every heap value reachable from them.
    pub(crate) fn visit(&mut self, heap: &Heap, vals: impl Iterator<Item = Val>) {
        for val in vals {
            self.visit_val(heap, val);
        }
    }

    /// Mark a module and every heap value reachable from its globals.
    pub(crate) fn visit_module(&mut self, heap: &Heap, handle: HeapHandle<Module>) {
        if !self.modules.insert(handle.index()) {
            return;
        }
        if let Some(module) = heap.modules.get(handle) {
            self.visit(heap, module.values());
        }
    }

    fn visit_val(&mut self, heap: &Heap, val: Val) {
        match val {
            Val::String(handle) => {
                self.strings.insert(handle.index());
            }
            Val::Pair(handle) => self.visit_pair(heap, handle),
            Val::NativeClosure(handle) => {
                self.native_closures.insert(handle.index());
            }
            Val::BytecodeFunction(handle) => self.visit_bytecode_function(heap, handle),
            Val::Closure(closure) => {
                self.visit_closure(heap, closure);
            }
            Val::Undefined
            | Val::Bool(_)
            | Val::Nil
            | Val::Int(_)
            | Val::Float(_)
            | Val::Symbol(_)
            | Val::NativeFunction(_) => {}
        }
    }

    fn visit_pair(&mut self, heap: &Heap, mut handle: HeapHandle<Pair>) {
        loop {
            if !self.pairs.insert(handle.index()) {
                return;
            }
            let Some(pair) = heap.pairs.get(handle) else {
                return;
            };
            self.visit_val(heap, pair.car);
            match pair.cdr {
                Val::Pair(h) => handle = h,
                Val::Nil => return,
                _ => {
                    self.visit_val(heap, pair.cdr);
                    return;
                }
            }
        }
    }

    fn visit_boxed_value(&mut self, heap: &Heap, handle: HeapHandle<BoxedValue>) {
        if !self.boxed_values.insert(handle.index()) {
            return;
        }
        let value = heap.boxed_values.get(handle).map(BoxedValue::value);
        if let Some(value) = value {
            self.visit_val(heap, value);
        }
    }

    fn visit_closure(&mut self, heap: &Heap, handle: HeapHandle<ClosureCaptures>) {
        if !self.closures.insert(handle.index()) {
            return;
        }
        let Some(closure) = heap.closures.get(handle) else {
            return;
        };
        self.visit_bytecode_function(heap, closure.function);
        for capture in closure.values.iter() {
            match *capture {
                CaptureStorage::Boxed(boxed) | CaptureStorage::ReadOnlyBoxed(boxed) => {
                    self.visit_boxed_value(heap, boxed)
                }
                CaptureStorage::Stack(_) => {}
            }
        }
    }

    fn visit_bytecode_function(&mut self, heap: &Heap, handle: HeapHandle<BytecodeFunction>) {
        if !self.bytecode_functions.insert(handle.index()) {
            return;
        }
        let Some(function) = heap.bytecode_functions.get(handle) else {
            return;
        };
        self.visit_module(heap, function.module);
        for instruction in function.bytecode.iter() {
            match *instruction {
                Instruction::PushConst(value) => self.visit_val(heap, value),
                Instruction::MakeClosure(function) => self.visit_bytecode_function(heap, function),
                _ => {}
            }
        }
    }

    /// Delete every heap entry that was not marked during [`Self::visit`].
    pub(crate) fn finalize(&mut self, heap: &mut Heap) {
        heap.strings
            .retain(|handle| self.strings.contains(&handle.index()));
        heap.pairs
            .retain(|handle| self.pairs.contains(&handle.index()));
        heap.bytecode_functions
            .retain(|handle| self.bytecode_functions.contains(&handle.index()));
        heap.boxed_values
            .retain(|handle| self.boxed_values.contains(&handle.index()));
        heap.closures
            .retain(|handle| self.closures.contains(&handle.index()));
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::function::BytecodeFunction;
    use crate::heap::Heap;
    use crate::symbol::Symbol;

    #[test]
    fn function_and_module_roots_keep_module_globals_alive() {
        let mut heap = Heap::default();
        let module = heap.modules.insert(ModuleSource::Internal("test")).unwrap();
        let string = heap.strings.insert("live");
        let function = heap
            .bytecode_functions
            .insert(BytecodeFunction::new(module));
        let globals = heap.modules.get_mut(module).unwrap();
        globals.set(Symbol::must_inline("value"), Val::String(string));
        // The module and its function form a cycle.
        globals.set(Symbol::must_inline("func"), Val::BytecodeFunction(function));

        let mut gc = MarkAndSweepGarbageCollector::default();
        gc.visit(&heap, [Val::BytecodeFunction(function)].into_iter());
        gc.finalize(&mut heap);
        assert!(heap.modules.get(module).is_some());
        assert!(heap.strings.get(string).is_some());
        assert!(heap.bytecode_functions.get(function).is_some());

        // Modules are roots even when no function is supplied as a root.
        gc.visit_module(&heap, module);
        gc.finalize(&mut heap);
        assert!(heap.modules.get(module).is_some());
        assert!(heap.strings.get(string).is_some());
        assert!(heap.bytecode_functions.get(function).is_some());
    }

    #[test]
    fn visits_nested_and_cyclic_lists() {
        let mut heap = Heap::default();
        let first = heap.pairs.insert(Pair {
            car: Val::Nil,
            cdr: Val::Nil,
        });
        let second = heap.pairs.insert(Pair {
            car: Val::Pair(first),
            cdr: Val::Nil,
        });
        heap.pairs.get_mut(first).unwrap().car = Val::Pair(second);
        let unused = heap.pairs.insert(Pair {
            car: Val::Nil,
            cdr: Val::Nil,
        });

        let mut gc = MarkAndSweepGarbageCollector::default();
        gc.visit(&heap, [Val::Pair(first)].into_iter());
        gc.finalize(&mut heap);

        assert!(heap.pairs.get(first).is_some());
        assert!(heap.pairs.get(second).is_some());
        assert!(heap.pairs.get(unused).is_none());
    }
}
