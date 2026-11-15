use smol_str::SmolStr;

pub mod function;

use function::{BytecodeFunction, NativeFunction};

/// Contains a value from the Vm.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Val {
    #[default]
    Nil,
    Undefined,
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
    ModuleNotFound(crate::ModuleId),
    ImportNotFound {
        module: crate::ModuleId,
        alias: Symbol,
    },
    ImportNotAtTopLevel,
    SymbolNotFound {
        module: crate::ModuleId,
        name: Symbol,
    },
    ParseError(crate::ast::AstBuilderError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn false_bool_is_falsey() {
        assert!(Val::Bool(false).is_falsey());
    }

    #[test]
    fn non_false_values_are_not_falsey() {
        assert!(!Val::Bool(true).is_falsey());
        assert!(!Val::Nil.is_falsey());
        assert!(!Val::Int(0).is_falsey());
        assert!(!Val::Float(0.0).is_falsey());
        assert!(!Val::String(String::new()).is_falsey());
        assert!(!Val::Symbol("false".into()).is_falsey());
        assert!(!Val::List(vec![]).is_falsey());
    }
}
