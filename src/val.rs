use smol_str::SmolStr;

pub mod function;

use function::{BytecodeFunction, NativeFunction};

/// Contains a value from the Vm.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Symbol(Symbol),
    List(Vec<Val>),
    NativeFunction(&'static NativeFunction),
    BytecodeFunction(BytecodeFunction),
}

impl Val {
    pub fn is_falsey(&self) -> bool {
        matches!(self, Val::Bool(false))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Symbol(pub(crate) SmolStr);

impl From<&str> for Symbol {
    fn from(s: &str) -> Symbol {
        Symbol(s.into())
    }
}

#[non_exhaustive]
#[derive(Debug, PartialEq)]
pub enum VmError {
    StackUnderflow,
    StackOverflow,
    NotCallable(Val),
    InvalidType { expected: &'static str, actual: Val },
}
