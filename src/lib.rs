use std::{collections::HashMap, path::PathBuf, sync::Arc};

use crate::{
    ast::Ast,
    builtins::PLUS_FN,
    compiler::Compiler,
    val::{Symbol, Val, VmError, function::Instruction},
};
/// Parse text into AST (abstract syntax tree).
pub mod ast;
/// A collection of builtin native functions.
mod builtins;
/// Compile Ast into bytecode.
pub mod compiler;
/// Parse text into datastructures.
pub mod reader;
/// Tokenize text into stream of Lisp tokens.
mod tokenizer;
/// Defines the value type that can hold an object of any type.
pub mod val;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ModuleId {
    Internal(Symbol),
    File(PathBuf),
}

impl ModuleId {
    pub fn internal(name: impl Into<Symbol>) -> Self {
        Self::Internal(name.into())
    }

    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self::File(path.into())
    }
}

impl Default for ModuleId {
    fn default() -> Self {
        Self::Internal("user".into())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Module {
    id: ModuleId,
    globals: HashMap<Symbol, Val>,
}

impl Module {
    pub fn new(id: ModuleId) -> Self {
        Self {
            id,
            globals: HashMap::new(),
        }
    }

    pub fn internal(name: impl Into<Symbol>) -> Self {
        Self::new(ModuleId::internal(name))
    }

    pub fn from_file(path: impl Into<PathBuf>) -> Self {
        Self::new(ModuleId::file(path))
    }

    pub fn id(&self) -> &ModuleId {
        &self.id
    }

    pub fn get(&self, symbol: &Symbol) -> Option<&Val> {
        self.globals.get(symbol)
    }

    pub fn insert(&mut self, symbol: impl Into<Symbol>, val: Val) -> Option<Val> {
        self.globals.insert(symbol.into(), val)
    }
}

#[derive(Default)]
pub struct Vm {
    module: Module,
    stack: Vec<Val>,
}

#[derive(Copy, Clone, Debug)]
enum ReturnAddress {
    Stack(usize),
    Exit,
}

#[derive(Debug)]
struct CallFrame {
    return_address: ReturnAddress,
    instructions: Arc<[Instruction]>,
    instruction_idx: usize,
    stack_start: usize,
}

impl Vm {
    pub fn new() -> Vm {
        let mut vm = Vm::default();
        vm.module.insert("+", Val::NativeFunction(&PLUS_FN));
        vm
    }

    pub fn eval(&mut self, func: Val, args: impl IntoIterator<Item = Val>) -> Result<Val, VmError> {
        self.stack.clear();
        self.stack.extend(std::iter::once(func).chain(args));
        let arg_count = self.stack.len() - 1;
        let mut call_frames = Vec::with_capacity(64);
        call_frames.push(CallFrame {
            return_address: ReturnAddress::Exit,
            instructions: [Instruction::Eval(arg_count)].into(),
            instruction_idx: 0,
            stack_start: 0,
        });
        self.run(&mut call_frames)
    }

    pub fn eval_str(&mut self, source: &str) -> Result<Val, VmError> {
        let ast = Ast::parse(source).unwrap();
        let instructions = Compiler::default().compile_to_instructions(&ast).into();
        self.stack.clear();
        let mut call_frames = Vec::with_capacity(64);
        call_frames.push(CallFrame {
            return_address: ReturnAddress::Exit,
            instructions,
            instruction_idx: 0,
            stack_start: 0,
        });
        self.run(&mut call_frames)
    }

    fn run(&mut self, call_frames: &mut Vec<CallFrame>) -> Result<Val, VmError> {
        let Some(mut call_frame) = call_frames.last_mut() else {
            return Err(VmError::StackUnderflow);
        };
        loop {
            let instruction = call_frame
                .instructions
                .get(call_frame.instruction_idx)
                .unwrap_or(&Instruction::Return);
            call_frame.instruction_idx += 1;
            match instruction {
                Instruction::PushConst(val) => self.stack.push(val.clone()),
                Instruction::LookupGlobal(sym) => {
                    let val = self.module.get(sym).unwrap();
                    self.stack.push(val.clone());
                }
                Instruction::LookupLocal(idx) => {
                    let absolute_idx = idx + call_frame.stack_start;
                    let val = self
                        .stack
                        .get(absolute_idx)
                        .ok_or(VmError::StackUnderflow)?;
                    self.stack.push(val.clone());
                }
                Instruction::Eval(n) => {
                    let n = *n;
                    call_frame = self.execute_eval(n, call_frames)?;
                }
                Instruction::Pop(n) => {
                    let new_len = self.stack.len().saturating_sub(*n);
                    self.stack.truncate(new_len);
                }
                Instruction::Jump(n) => call_frame.instruction_idx += *n,
                Instruction::JumpIfNot(n) => {
                    let val = self.stack.pop().ok_or(VmError::StackUnderflow)?;
                    if val.is_falsey() {
                        call_frame.instruction_idx += *n;
                    }
                }
                Instruction::Return => {
                    let return_val = self.stack.pop().ok_or(VmError::StackUnderflow)?;
                    let returning_frame = call_frames.pop().ok_or(VmError::StackUnderflow)?;
                    match returning_frame.return_address {
                        ReturnAddress::Stack(idx) => self.stack[idx] = return_val,
                        ReturnAddress::Exit => return Ok(return_val),
                    }
                    self.stack.truncate(returning_frame.stack_start);
                    call_frame = call_frames.last_mut().ok_or(VmError::StackUnderflow)?;
                }
            }
        }
    }

    fn execute_eval<'a>(
        &mut self,
        n: usize,
        call_frames: &'a mut Vec<CallFrame>,
    ) -> Result<&'a mut CallFrame, VmError> {
        let stack_start = self.stack.len() - n;
        let func_idx = stack_start - 1;
        let func_val = self.stack[func_idx].clone();
        match func_val {
            Val::NativeFunction(native_function) => {
                let args = &self.stack[stack_start..];
                let ret = native_function.eval(args)?;
                self.stack[func_idx] = ret;
                self.stack.truncate(stack_start);
                call_frames.last_mut().ok_or(VmError::StackUnderflow)
            }
            Val::BytecodeFunction(bytecode_function) => {
                let call_frame = CallFrame {
                    instructions: bytecode_function.instructions,
                    instruction_idx: 0,
                    stack_start,
                    return_address: ReturnAddress::Stack(func_idx),
                };
                call_frames.push(call_frame);
                call_frames.last_mut().ok_or(VmError::StackUnderflow)
            }
            _ => Err(VmError::NotCallable(func_val)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_is_called() {
        let mut vm = Vm::new();
        assert_eq!(vm.eval_str("(+ 1 2 (+ 3 4))"), Ok(Val::Int(10)));
    }

    #[test]
    fn if_returns_correct_branch() {
        let mut vm = Vm::new();
        assert_eq!(vm.eval_str("(if #true (if #false 1 2) 3)"), Ok(Val::Int(2)));
    }

    #[test]
    fn lambda_is_called() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str("((lambda (a) (+ 1 2 a) (+ 1 2 3 4 a)) 10)"),
            Ok(Val::Int(20))
        );
    }

    #[test]
    fn module_stores_and_looks_up_globals() {
        let mut module = Module::default();
        assert_eq!(module.get(&Symbol::from("x")), None);
        assert_eq!(module.insert("x", Val::Int(42)), None);
        assert_eq!(module.get(&Symbol::from("x")), Some(&Val::Int(42)));
        assert_eq!(module.insert("x", Val::Int(99)), Some(Val::Int(42)));
        assert_eq!(module.get(&Symbol::from("x")), Some(&Val::Int(99)));
    }

    #[test]
    fn module_tracks_internal_and_file_identity() {
        let default_mod = Module::default();
        assert_eq!(default_mod.id(), &ModuleId::internal("user"));

        let core_mod = Module::internal("core");
        assert_eq!(core_mod.id(), &ModuleId::Internal("core".into()));

        let file_mod = Module::from_file("examples/hello.tl");
        assert_eq!(
            file_mod.id(),
            &ModuleId::File(PathBuf::from("examples/hello.tl"))
        );
    }
}
