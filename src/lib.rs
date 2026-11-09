/// Parse text into datastructures.
pub mod reader;
/// Tokenize text into stream of Lisp tokens.
pub mod tokenizer;

/// Contains a value from the Vm.
#[derive(Debug, PartialEq)]
pub enum Val {
    String(String),
    Symbol(String),
    List(Vec<Val>),
}

pub struct Vm {}
