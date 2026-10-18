//! Function representations used by the takolisp VM.

use std::sync::Arc;

use crate::boxed_value::BoxedValue;
use crate::error::RuntimeError;
use crate::heap::HeapHandle;
use crate::module::{Module, ModuleSlot};
use crate::symbol::Symbol;
use crate::val::Val;

/// An index used by bytecode to refer to a module slot.
pub type SlotIndex = ModuleSlot;

/// An index into the values captured by a closure.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct UpvalueSlot(usize);

impl UpvalueSlot {
    /// Creates a slot referring to `index`.
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    /// Returns the underlying capture index.
    pub const fn index(self) -> usize {
        self.0
    }
}

/// The number of arguments accepted by a function.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Arity {
    /// Any number of arguments.
    #[default]
    Any,
    /// The function accepts exactly this number of arguments.
    Exact(u32),
    /// The function accepts at least this number of arguments.
    AtLeast(u32),
}

#[derive(Clone, Debug, PartialEq)]
/// A compiled takolisp function.
pub struct BytecodeFunction {
    /// The module in which the function was compiled.
    pub module: HeapHandle<Module>,
    /// The name of the function.
    pub name: Option<Symbol>,
    /// The exact number of arguments accepted by the function.
    pub arity: Arity,
    /// The number of local slots reserved in addition to argument slots.
    pub local_count: u32,
    /// Descriptors for the values captured from the enclosing function.
    pub captures: Arc<[CaptureSource]>,
    /// The instructions executed by the VM.
    pub bytecode: Arc<[Instruction]>,
}

impl BytecodeFunction {
    /// Creates an empty function belonging to `module`.
    pub fn new(module: HeapHandle<Module>) -> Self {
        Self {
            module,
            name: None,
            arity: Arity::default(),
            local_count: 0,
            captures: Arc::default(),
            bytecode: Arc::default(),
        }
    }
}

/// Describes how a function obtains one of its captured values.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CaptureSource {
    /// Capture a local or argument from the enclosing function's frame.
    Local(usize),
    /// Capture an upvalue from the enclosing function.
    Upvalue(usize),
    /// Capture the enclosing function itself.
    CurrentFunction,
}

/// Describes where a value captured by a closure is stored.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum CaptureStorage {
    /// An absolute index into the VM stack.
    Stack(usize),
    /// A mutable heap-allocated value.
    Boxed(HeapHandle<BoxedValue>),
    /// A heap-allocated value that must not be mutated through the closure.
    ReadOnlyBoxed(HeapHandle<BoxedValue>),
}

/// The values captured by a closure.
#[derive(Clone, Debug, PartialEq)]
pub struct ClosureCaptures {
    /// The function for the closure.
    pub function: HeapHandle<BytecodeFunction>,
    /// The locations of the captured values.
    pub values: Box<[CaptureStorage]>,
}

impl ClosureCaptures {
    /// Create a collection of captured values.
    pub fn from_captures(
        function: HeapHandle<BytecodeFunction>,
        values: Box<[CaptureStorage]>,
    ) -> Self {
        Self { function, values }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
/// An instruction executed by the takolisp VM.
pub enum Instruction {
    /// Pushes a constant value onto the evaluation stack.
    PushConst(Val),
    /// Looks up a slot in the current call frame's module and pushes its value.
    LookupSlot(ModuleSlot),
    /// Creates a closure from a compiled function and the current environment.
    MakeClosure(HeapHandle<BytecodeFunction>),
    /// Gets an upvalue by index and pushes it onto the stack.
    GetUpvalue(UpvalueSlot),
    /// Sets the value in an upvalue.
    SetUpvalue(UpvalueSlot),
    /// Pushes a local value onto the stack.
    GetLocal(usize),
    /// Pushes the function onto the stack.
    GetCurrentFunction,
    /// Moves the value from the top of the stack in a local slot.
    SetLocal(usize),
    /// Defines a slot in the current call frame's module using the value on top
    /// of the stack and replaces that value with undefined.
    Define(SlotIndex),
    /// Evaluates a function using the specified number of stack arguments.
    Eval(usize),
    /// Removes the specified number of values from the top of the stack.
    PopN(usize),
    /// Jumps to an absolute instruction index when the top value is false.
    JumpIfFalse(usize),
    /// Jumps to an absolute instruction index.
    Jump(usize),
    /// Returns the top value from the current function.
    Return,
}

impl Arity {
    /// Validates the number of arguments supplied to a function.
    pub fn validate(self, found: usize) -> Result<(), RuntimeError> {
        match self {
            Arity::Any => {}
            Arity::Exact(expected) => {
                if expected as usize != found {
                    std::hint::cold_path();
                    return Err(RuntimeError::ArityError {
                        expected: self,
                        found,
                    });
                }
            }
            Arity::AtLeast(expected) => {
                if found < expected as usize {
                    std::hint::cold_path();
                    return Err(RuntimeError::ArityError {
                        expected: self,
                        found,
                    });
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Arity;
    use crate::error::RuntimeError;

    #[test]
    fn at_least_validates_minimum_argument_count() {
        assert_eq!(Arity::AtLeast(2).validate(2), Ok(()));
        assert_eq!(Arity::AtLeast(2).validate(3), Ok(()));
        assert_eq!(
            Arity::AtLeast(2).validate(1),
            Err(RuntimeError::ArityError {
                expected: Arity::AtLeast(2),
                found: 1,
            })
        );
    }
}
