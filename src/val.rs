use crate::Vm;
use crate::function::{BytecodeFunction, ClosureCaptures};
use crate::heap::HeapHandle;
use crate::native_function::{NativeClosure, NativeFunction};
use crate::pair::Pair;
use crate::symbol::Symbol;

/// Contains a value from the `Vm`.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum Val {
    /// A value that has not been defined.
    #[default]
    Undefined,
    /// A boolean value.
    Bool(bool),
    /// The Lisp nil value.
    Nil,
    /// A signed integer value.
    Int(i64),
    /// A floating-point value.
    Float(f64),
    /// An interned symbol.
    Symbol(Symbol),
    /// A handle to a heap-allocated string.
    String(HeapHandle<String>),
    /// A handle to a heap-allocated list of values.
    Pair(HeapHandle<Pair>),
    /// A built-in function implemented in Rust.
    NativeFunction(&'static NativeFunction),
    /// A closure implemented in Rust.
    NativeClosure(HeapHandle<NativeClosure>),
    /// A handle to a heap-allocated compiled function.
    BytecodeFunction(HeapHandle<BytecodeFunction>),
    /// A handle to a heap-allocated compiled function and its captured values.
    Closure(HeapHandle<ClosureCaptures>),
}

/// Generates an [`impl From`] that wraps a value in a [`Val`] variant.
macro_rules! val_from {
    ($from:ty => $variant:ident) => {
        impl From<$from> for $crate::val::Val {
            fn from(value: $from) -> Self {
                $crate::val::Val::$variant(value)
            }
        }
    };
}

val_from!(bool => Bool);
val_from!(i64 => Int);
val_from!(f64 => Float);
val_from!(Symbol => Symbol);
val_from!(HeapHandle<String> => String);
val_from!(HeapHandle<Pair> => Pair);
val_from!(&'static NativeFunction => NativeFunction);
val_from!(HeapHandle<NativeClosure> => NativeClosure);
val_from!(HeapHandle<BytecodeFunction> => BytecodeFunction);

impl Val {
    /// Returns `true` when the value has not been defined.
    ///
    /// # Examples
    ///
    /// ```
    /// use takolisp::Val;
    ///
    /// assert!(Val::Undefined.is_undefined());
    /// assert!(Val::default().is_undefined());
    /// assert!(!Val::Nil.is_undefined());
    /// ```
    pub const fn is_undefined(self) -> bool {
        matches!(self, Self::Undefined)
    }

    /// Get a type that can be used to display a value using the VM's heap.
    pub fn display<'a>(self, vm: &'a Vm) -> impl std::fmt::Display + std::fmt::Debug + 'a {
        DisplayVal { val: self, vm }
    }
}

#[derive(Copy, Clone)]
struct DisplayVal<'a> {
    vm: &'a Vm,
    val: Val,
}

impl<'a> DisplayVal<'a> {
    /// Format a proper or improper list, walking the pair chain.
    fn fmt_pair_val<const DEBUG: bool>(
        vm: &'a Vm,
        mut current: Pair,
        depth: usize,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        f.write_str("(")?;
        loop {
            DisplayVal {
                vm,
                val: current.car,
            }
            .fmt_val::<DEBUG>(depth + 1, f)?;
            match current.cdr {
                Val::Nil => break,
                Val::Pair(next_handle) => {
                    f.write_str(" ")?;
                    let Some(next) = vm.heap().pairs.get(next_handle).copied() else {
                        return f.write_str(" . <invalid-pair>)");
                    };
                    current = next;
                }
                other => {
                    f.write_str(" . ")?;
                    DisplayVal { vm, val: other }.fmt_val::<DEBUG>(depth + 1, f)?;
                    break;
                }
            }
        }
        f.write_str(")")
    }

    fn fmt_val<const DEBUG: bool>(
        self,
        depth: usize,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        const MAX_DISPLAY_DEPTH: usize = 10;
        if depth >= MAX_DISPLAY_DEPTH {
            std::hint::cold_path();
            return f.write_str("...");
        }

        match self.val {
            Val::Undefined => f.write_str("undefined"),
            Val::Bool(value) => {
                if DEBUG {
                    write!(f, "#{value}")
                } else {
                    write!(f, "{value}")
                }
            }
            Val::Nil => f.write_str("nil"),
            Val::Int(value) => write!(f, "{value}"),
            Val::Float(value) => write!(f, "{value}"),
            Val::Symbol(symbol) => {
                std::fmt::Display::fmt(&self.vm.heap().symbols.display_symbol(symbol), f)
            }
            Val::String(handle) => match self.vm.heap().strings.get(handle) {
                Some(string) => {
                    if DEBUG {
                        write!(f, "{string:?}")
                    } else {
                        f.write_str(string)
                    }
                }
                None => f.write_str("<invalid-string>"),
            },
            Val::Pair(handle) => match self.vm.heap().pairs.get(handle) {
                Some(pair) => Self::fmt_pair_val::<DEBUG>(self.vm, *pair, depth, f),
                None => f.write_str("<invalid-pair>"),
            },
            Val::NativeFunction(function) => {
                write!(f, "<native-func {name}>", name = function.name)
            }
            Val::NativeClosure(handle) => {
                let Some(closure) = self.vm.heap().native_closures.get(handle) else {
                    return f.write_str("<invalid-native-closure>");
                };
                write!(f, "<native-closure {name}>", name = closure.name)
            }
            Val::BytecodeFunction(handle) => {
                let Some(function) = self.vm.heap().bytecode_functions.get(handle) else {
                    return f.write_str("<invalid-func>");
                };
                match function.name {
                    Some(name) => write!(
                        f,
                        "<func {name}>",
                        name = self.vm.heap().symbols.display_symbol(name)
                    ),
                    None => f.write_str("<anonymous func>"),
                }
            }
            Val::Closure(closure_handle) => {
                let Some(closure) = self.vm.heap().closures.get(closure_handle) else {
                    return f.write_str("<invalid-closure>");
                };
                let Some(function) = self.vm.heap().bytecode_functions.get(closure.function) else {
                    return f.write_str("<invalid-closure>");
                };
                let Some(name_sym) = function.name.as_ref() else {
                    return f.write_str("<anonymous closure>");
                };
                let display_sym = self.vm.heap().symbols.display_symbol(*name_sym);
                write!(f, "<closure {display_sym}>",)
            }
        }
    }
}

impl std::fmt::Display for DisplayVal<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.fmt_val::<false>(0, f)
    }
}

impl std::fmt::Debug for DisplayVal<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.fmt_val::<true>(0, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn val_is_small() {
        assert_eq!(std::mem::size_of::<Val>(), 2 * std::mem::size_of::<u64>());
    }

    #[test]
    fn undefined_is_the_default_value() {
        assert_eq!(Val::default(), Val::Undefined);
        assert!(Val::default().is_undefined());
        assert!(!Val::Nil.is_undefined());
        assert!(!Val::Int(0).is_undefined());
    }

    fn cons(vm: &mut Vm, car: Val, cdr: Val) -> Val {
        Val::Pair(vm.heap_mut().pairs.insert(Pair { car, cdr }))
    }

    #[test]
    fn display_formats_values_using_their_source_representation() {
        let mut vm = Vm::new();
        let string = vm.heap_mut().strings.insert("hello\nworld");
        // Build `(42 "hello\nworld" nil)` by prepending elements right-to-left.
        let list = cons(&mut vm, Val::Nil, Val::Nil);
        let list = cons(&mut vm, Val::String(string), list);
        let list = cons(&mut vm, Val::Int(42), list);

        assert_eq!(Val::Int(42).display(&vm).to_string(), "42");
        assert_eq!(Val::String(string).display(&vm).to_string(), "hello\nworld");
        assert_eq!(list.display(&vm).to_string(), "(42 hello\nworld nil)");
        assert_eq!(
            format!("{:?}", Val::String(string).display(&vm)),
            "\"hello\\nworld\""
        );
        assert_eq!(Val::Bool(true).display(&vm).to_string(), "true");
        assert_eq!(format!("{:?}", Val::Bool(true).display(&vm)), "#true");
        assert_eq!(format!("{:?}", Val::Bool(false).display(&vm)), "#false");
    }

    #[test]
    fn display_limits_nested_lists() {
        let mut vm = Vm::new();
        let mut val = Val::Nil;
        for _ in 0..=20 {
            val = Val::Pair(vm.heap_mut().pairs.insert(Pair {
                car: val,
                cdr: Val::Nil,
            }));
        }

        assert_eq!(val.display(&vm).to_string(), "((((((((((...))))))))))");
    }

    #[test]
    fn display_formats_functions_as_opaque_values() {
        let mut vm = Vm::new();

        let native = vm
            .heap()
            .modules
            .get(vm.global_module())
            .unwrap()
            .get(Symbol::must_inline("+"));
        let bytecode = vm.eval_str(vm.global_module(), "(lambda (x) x)").unwrap();

        assert_eq!(native.display(&vm).to_string(), "<native-func +>");
        assert_eq!(format!("{:?}", native.display(&vm)), "<native-func +>");
        assert_eq!(bytecode.display(&vm).to_string(), "<anonymous func>");
    }

    #[test]
    fn display_formats_closures_as_opaque_values() {
        let mut vm = Vm::new();
        let module = vm.global_module();
        let function = vm.heap_mut().bytecode_functions.insert(BytecodeFunction {
            module,
            name: Some(Symbol::must_inline("answer")),
            ..BytecodeFunction::new(module)
        });
        let captures = vm
            .heap_mut()
            .closures
            .insert(ClosureCaptures::from_captures(function, Default::default()));
        let closure = Val::Closure(captures);

        assert_eq!(closure.display(&vm).to_string(), "<closure answer>");
    }
}
