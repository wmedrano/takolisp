use std::sync::Arc;

use crate::{
    ModuleId,
    val::{Symbol, Val, VmError},
};

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
    pub(crate) module: ModuleId,
    pub(crate) name: Option<Symbol>,
    pub(crate) parameters: Vec<Symbol>,
    pub(crate) instructions: Arc<[Instruction]>,
}

impl BytecodeFunction {
    pub fn module(&self) -> &ModuleId {
        &self.module
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Instruction {
    PushConst(Val),
    Define(Symbol),
    Lookup(ModuleId, Symbol),
    LookupLocal(usize),
    Eval(usize),
    Pop(usize),
    Jump(usize),
    JumpIfNot(usize),
    Return,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop(_: &[Val]) -> Result<Val, VmError> {
        Ok(Val::Nil)
    }

    #[test]
    fn native_functions_compare_equal_only_by_identity() {
        static FN_A: NativeFunction = NativeFunction::new(noop);
        static FN_B: NativeFunction = NativeFunction::new(noop);

        assert_eq!(FN_A, FN_A);
        assert_ne!(FN_A, FN_B);
    }
}
