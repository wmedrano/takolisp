use std::sync::Arc;

use crate::val::{Val, VmError};

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
    Jump(usize),
    JumpIfNot(usize),
    Return,
}
