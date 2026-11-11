use std::sync::Arc;

/// Contains a value from the Vm.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Symbol(String),
    List(Vec<Val>),
    NativeFunction(&'static NativeFunction),
    BytecodeFunction(BytecodeFunction),
}

#[derive(Debug, PartialEq)]
pub enum VmError {
    StackUnderflow,
    StackOverflow,
    NotCallable(Val),
    InvalidType { expected: &'static str, actual: Val },
}

pub struct NativeFunction {
    func: fn(&[Val]) -> Result<Val, VmError>,
}

impl NativeFunction {
    pub const fn new(func: fn(&[Val]) -> Result<Val, VmError>) -> NativeFunction {
        NativeFunction { func }
    }

    pub fn eval(&self, args: &[Val]) -> Result<Val, VmError> {
        (self.func)(args)
    }
}

impl std::fmt::Debug for NativeFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeFunction").finish_non_exhaustive()
    }
}

impl PartialEq for NativeFunction {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BytecodeFunction {
    pub(crate) instructions: Arc<[Instruction]>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Instruction {
    PushConst(Val),
    Lookup(String),
    Eval(usize),
    Pop(usize),
    Return,
}
