/// Parse text into AST (abstract syntax tree).
pub mod ast;
/// Parse text into datastructures.
pub mod reader;
/// Tokenize text into stream of Lisp tokens.
pub mod tokenizer;

/// Contains a value from the Vm.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Nil,
    Int(i64),
    Float(f64),
    String(String),
    Symbol(String),
    List(Vec<Val>),
}

pub struct Vm {}
