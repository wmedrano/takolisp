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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Symbol(pub(crate) SmolStr);

impl From<&str> for Symbol {
    fn from(s: &str) -> Symbol {
        Symbol(s.into())
    }
}

impl std::fmt::Display for Symbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Val {
    pub fn is_falsey(&self) -> bool {
        matches!(self, Val::Bool(false))
    }

    fn display(&self, f: &mut std::fmt::Formatter<'_>, max_depth: usize) -> std::fmt::Result {
        match self {
            Val::Nil => f.write_str("nil"),
            Val::Undefined => f.write_str("#<undefined>"),
            Val::Bool(true) => f.write_str("#true"),
            Val::Bool(false) => f.write_str("#false"),
            Val::Int(n) => write!(f, "{n}"),
            Val::Float(n) => write!(f, "{n}"),
            Val::String(s) => write!(f, "{s:?}"),
            Val::Symbol(sym) => write!(f, "{sym}"),
            Val::List(items) => {
                if items.is_empty() {
                    return f.write_str("()");
                }
                if max_depth == 0 {
                    return f.write_str("(...)");
                }
                f.write_str("(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" ")?;
                    }
                    item.display(f, max_depth - 1)?;
                }
                f.write_str(")")
            }
            Val::NativeFunction(_) => f.write_str("#<native-fn>"),
            Val::BytecodeFunction(func) => match &func.name {
                Some(name) => write!(f, "#<fn {name}>"),
                None => f.write_str("#<fn>"),
            },
        }
    }
}

impl std::fmt::Display for Val {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let max_depth = 10;
        self.display(f, max_depth)
    }
}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IoError {
    ReadFile {
        path: std::path::PathBuf,
        kind: std::io::ErrorKind,
    },
}

impl From<IoError> for VmError {
    fn from(err: IoError) -> Self {
        VmError::IoError(err)
    }
}

#[non_exhaustive]
#[derive(Debug, PartialEq)]
pub enum VmError {
    StackUnderflow,
    StackOverflow,
    NotCallable(Val),
    InvalidType {
        expected: &'static str,
        actual: Val,
    },
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
    IoError(IoError),
    CircularImport(std::path::PathBuf),
    ModuleLoadError {
        path: std::path::PathBuf,
        error: Box<VmError>,
    },
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

    #[test]
    fn primitives_display_as_lisp_literals() {
        assert_eq!(Val::Nil.to_string(), "nil");
        assert_eq!(Val::Undefined.to_string(), "#<undefined>");
        assert_eq!(Val::Bool(true).to_string(), "#true");
        assert_eq!(Val::Bool(false).to_string(), "#false");
        assert_eq!(Val::Int(-42).to_string(), "-42");
        assert_eq!(Val::Float(3.5).to_string(), "3.5");
        assert_eq!(Val::String("a\nb".into()).to_string(), r#""a\nb""#);
        assert_eq!(Val::Symbol("foo".into()).to_string(), "foo");
    }

    #[test]
    fn functions_display_with_tag_and_optional_name() {
        fn noop(_: &[Val]) -> Result<Val, VmError> {
            Ok(Val::Nil)
        }
        static NATIVE: NativeFunction = NativeFunction::new(noop);
        assert_eq!(Val::NativeFunction(&NATIVE).to_string(), "#<native-fn>");

        let named_fn = BytecodeFunction {
            module: crate::ModuleId::internal("test"),
            name: Some("add".into()),
            parameters: vec![],
            instructions: std::sync::Arc::new([]),
        };
        assert_eq!(Val::BytecodeFunction(named_fn).to_string(), "#<fn add>");

        let anon_fn = BytecodeFunction {
            module: crate::ModuleId::internal("test"),
            name: None,
            parameters: vec![],
            instructions: std::sync::Arc::new([]),
        };
        assert_eq!(Val::BytecodeFunction(anon_fn).to_string(), "#<fn>");
    }

    #[test]
    fn lists_display_with_parentheses() {
        let empty = Val::List(vec![]);
        let nested = Val::List(vec![
            Val::Int(1),
            Val::List(vec![Val::Int(2), Val::List(vec![Val::Int(3)])]),
        ]);

        assert_eq!(empty.to_string(), "()");
        assert_eq!(nested.to_string(), "(1 (2 (3)))");
    }

    #[test]
    fn nested_lists_beyond_max_depth_truncate_with_ellipsis() {
        let mut deep = Val::Int(1);
        for _ in 0..100 {
            deep = Val::List(vec![deep]);
        }

        assert!(deep.to_string().contains("(...)"));
    }
}
