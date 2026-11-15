use std::{collections::HashMap, sync::Arc};

use crate::{
    ast::{Ast, AstBuilderError},
    builtins::PLUS_FN,
    compiler::{Compiler, CompilerError},
    val::{Val, VmError, function::Instruction},
};
/// Parse text into AST (abstract syntax tree).
pub mod ast;
/// A collection of builtin native functions.
mod builtins;
/// Compile Ast into bytecode.
pub mod compiler;
/// Module identity and global symbol storage.
pub mod module;
/// Parse text into datastructures.
pub mod reader;
/// Tokenize text into stream of Lisp tokens.
mod tokenizer;
/// Defines the value type that can hold an object of any type.
pub mod val;

pub use module::{Module, ModuleId};

impl From<AstBuilderError> for VmError {
    fn from(err: AstBuilderError) -> Self {
        VmError::ParseError(err)
    }
}

impl From<CompilerError> for VmError {
    fn from(err: CompilerError) -> Self {
        match err {
            CompilerError::ModuleNotFound(id) => VmError::ModuleNotFound(id),
            CompilerError::ImportNotFound { module, alias } => {
                VmError::ImportNotFound { module, alias }
            }
            CompilerError::ImportNotAtTopLevel => VmError::ImportNotAtTopLevel,
        }
    }
}

#[derive(Default)]
pub struct Vm {
    modules: HashMap<ModuleId, Module>,
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
    module: ModuleId,
    instructions: Arc<[Instruction]>,
    instruction_idx: usize,
    stack_start: usize,
}

impl Vm {
    pub fn new() -> Vm {
        let mut vm = Vm::default();
        let mut global_module = Module::new(ModuleId::GLOBAL);
        global_module.insert("+", Val::NativeFunction(&PLUS_FN));
        vm.modules.insert(ModuleId::GLOBAL, global_module);
        vm.module_or_init(&ModuleId::USER);
        vm
    }

    pub fn module(&self, id: &ModuleId) -> Option<&Module> {
        self.modules.get(id)
    }

    fn module_or_init(&mut self, id: &ModuleId) -> &mut Module {
        self.modules
            .entry(id.clone())
            .or_insert_with(|| Module::new(id.clone()))
    }

    pub fn eval(&mut self, func: Val, args: impl IntoIterator<Item = Val>) -> Result<Val, VmError> {
        self.stack.clear();
        self.stack.extend(std::iter::once(func).chain(args));
        let arg_count = self.stack.len() - 1;
        let mut call_frames = Vec::with_capacity(64);
        call_frames.push(CallFrame {
            return_address: ReturnAddress::Exit,
            module: ModuleId::USER,
            instructions: [Instruction::Eval(arg_count)].into(),
            instruction_idx: 0,
            stack_start: 0,
        });
        self.run(&mut call_frames)
    }

    pub fn eval_str(&mut self, source: &str) -> Result<Val, VmError> {
        self.eval_str_in(ModuleId::USER, source)
    }

    pub fn eval_str_in(&mut self, module: ModuleId, source: &str) -> Result<Val, VmError> {
        self.eval_source_in(module, source, false)
    }

    pub fn load_file(&mut self, path: impl AsRef<std::path::Path>) -> Result<Val, VmError> {
        let path = path.as_ref();
        let source = std::fs::read_to_string(path).map_err(|err| VmError::IoError {
            path: path.to_path_buf(),
            kind: err.kind(),
        })?;
        self.eval_source_in(ModuleId::file(path), &source, true)
    }

    fn eval_source_in(
        &mut self,
        module: ModuleId,
        source: &str,
        as_module: bool,
    ) -> Result<Val, VmError> {
        self.module_or_init(&module);
        let ast = Ast::parse_in(module.clone(), source)?;
        let compiler = Compiler::for_module(module.clone(), &mut self.modules);
        let instructions = if as_module {
            compiler.compile_module(&ast)?
        } else {
            compiler.compile_to_instructions(&ast)?
        }
        .into();
        self.stack.clear();
        let mut call_frames = Vec::with_capacity(64);
        call_frames.push(CallFrame {
            return_address: ReturnAddress::Exit,
            module,
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
                Instruction::Define(sym) => {
                    let val = std::mem::take(self.stack.last_mut().ok_or(VmError::StackUnderflow)?);
                    self.modules
                        .get_mut(&call_frame.module)
                        .ok_or_else(|| VmError::ModuleNotFound(call_frame.module.clone()))?
                        .insert(sym.clone(), val);
                }
                Instruction::Lookup(module, sym) => {
                    let mod_ref = self
                        .modules
                        .get(module)
                        .ok_or_else(|| VmError::ModuleNotFound(module.clone()))?;
                    let val = mod_ref
                        .get(sym)
                        .filter(|v| !matches!(v, Val::Undefined))
                        .ok_or_else(|| VmError::SymbolNotFound {
                            module: module.clone(),
                            name: sym.clone(),
                        })?
                        .clone();
                    self.stack.push(val);
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
                Instruction::MarkLoaded(module) => {
                    self.modules
                        .get_mut(module)
                        .ok_or_else(|| VmError::ModuleNotFound(module.clone()))?
                        .set_loaded(true);
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
                    return_address: ReturnAddress::Stack(func_idx),
                    module: bytecode_function.module,
                    instructions: bytecode_function.instructions,
                    instruction_idx: 0,
                    stack_start,
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
    use crate::val::Symbol;

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
    fn evaluating_native_function_directly_returns_result() {
        let mut vm = Vm::new();
        let result = vm.eval(
            Val::NativeFunction(&PLUS_FN),
            [Val::Int(2), Val::Int(3), Val::Int(4)],
        );
        assert_eq!(result, Ok(Val::Int(9)));
    }

    #[test]
    fn evaluating_bytecode_function_directly_binds_args_and_returns_result() {
        let mut vm = Vm::new();
        let func = vm.eval_str("(lambda (a b) (+ a b))").unwrap();
        assert_eq!(vm.eval(func, [Val::Int(5), Val::Int(7)]), Ok(Val::Int(12)));
    }

    #[test]
    fn evaluating_non_callable_value_returns_not_callable_error() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval(Val::Int(42), []),
            Err(VmError::NotCallable(Val::Int(42)))
        );
        assert_eq!(vm.eval_str("(1 2)"), Err(VmError::NotCallable(Val::Int(1))));
    }

    #[test]
    fn if_without_else_returns_nil_when_predicate_is_false() {
        let mut vm = Vm::new();
        assert_eq!(vm.eval_str("(if #false 1)"), Ok(Val::Nil));
    }

    #[test]
    fn if_treats_non_false_values_as_truthy() {
        let mut vm = Vm::new();
        for pred in ["#true", "0", "\"\"", "(if #false 1)"] {
            assert_eq!(
                vm.eval_str(&format!("(if {pred} 1 2)")),
                Ok(Val::Int(1)),
                "predicate {pred} should be truthy"
            );
        }
    }

    #[test]
    fn if_skips_evaluating_untaken_branch() {
        let mut vm = Vm::new();
        assert_eq!(vm.eval_str("(if #true 1 (1))"), Ok(Val::Int(1)));
        assert_eq!(vm.eval_str("(if #false (1) 2)"), Ok(Val::Int(2)));
    }

    #[test]
    fn lambda_with_multiple_parameters_binds_positional_arguments() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str("((lambda (a b c) (+ a b)) 10 20 30)"),
            Ok(Val::Int(30))
        );
    }

    #[test]
    fn lambda_with_duplicate_parameter_uses_last_argument() {
        let mut vm = Vm::new();
        assert_eq!(vm.eval_str("((lambda (a a) a) 1 2)"), Ok(Val::Int(2)));
    }

    #[test]
    fn nested_calls_preserve_caller_locals() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str("((lambda (x) (+ x ((lambda (y) (+ y 1)) 10) x)) 5)"),
            Ok(Val::Int(21))
        );
    }

    #[test]
    fn sequential_evaluations_isolate_vm_stack() {
        let mut vm = Vm::new();
        assert_eq!(vm.eval_str("(+ 1 2)"), Ok(Val::Int(3)));
        assert_eq!(vm.eval_str("(+ 4 5)"), Ok(Val::Int(9)));
    }

    #[test]
    fn define_sets_global_and_returns_nil() {
        let mut vm = Vm::new();
        assert_eq!(vm.eval_str("(define x 42)"), Ok(Val::Nil));
        assert_eq!(vm.eval_str("x"), Ok(Val::Int(42)));
    }

    #[test]
    fn define_followed_by_expression_uses_defined_value() {
        let mut vm = Vm::new();
        assert_eq!(vm.eval_str("(define x 10) (+ x 5)"), Ok(Val::Int(15)));
    }

    #[test]
    fn define_function_shorthand_binds_callable_lambda() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str("(define (add-and-inc a b) (+ a b) (+ a b 1)) (add-and-inc 3 4)"),
            Ok(Val::Int(8))
        );
        assert_eq!(vm.eval_str("(define (noop)) (noop)"), Ok(Val::Nil));

        let Val::BytecodeFunction(named_fn) = vm.eval_str("add-and-inc").unwrap() else {
            panic!("expected bytecode function");
        };
        assert_eq!(named_fn.name, Some(Symbol::from("add-and-inc")));

        let Val::BytecodeFunction(anon_fn) = vm.eval_str("(lambda () 1)").unwrap() else {
            panic!("expected bytecode function");
        };
        assert_eq!(anon_fn.name, None);
    }

    #[test]
    fn eval_str_defaults_to_user_internal_module() {
        let mut vm = Vm::new();
        assert_eq!(vm.eval_str("(define x 42)"), Ok(Val::Nil));
        let user_module = vm.module(&ModuleId::internal("user")).unwrap();
        assert_eq!(user_module.get(&Symbol::from("x")), Some(&Val::Int(42)));
    }

    #[test]
    fn eval_str_in_isolates_definitions_per_module() {
        let mut vm = Vm::new();
        let mod_a = ModuleId::internal("mod_a");
        let mod_b = ModuleId::file("examples/mod_b.tl");

        assert_eq!(vm.eval_str_in(mod_a.clone(), "(define x 10)"), Ok(Val::Nil));
        assert_eq!(vm.eval_str_in(mod_b.clone(), "(define x 20)"), Ok(Val::Nil));

        assert_eq!(vm.eval_str_in(mod_a.clone(), "(+ x 1)"), Ok(Val::Int(11)));
        assert_eq!(vm.eval_str_in(mod_b.clone(), "(+ x 1)"), Ok(Val::Int(21)));
        assert_eq!(
            vm.module(&ModuleId::internal("user"))
                .unwrap()
                .get(&Symbol::from("x")),
            None
        );
    }

    #[test]
    fn bytecode_function_records_originating_module() {
        let mut vm = Vm::new();
        let Val::BytecodeFunction(user_fn) = vm.eval_str("(lambda (x) x)").unwrap() else {
            panic!("expected bytecode function");
        };
        assert_eq!(user_fn.module(), &ModuleId::internal("user"));

        let custom_mod = ModuleId::file("lib/math.tl");
        let Val::BytecodeFunction(custom_fn) = vm
            .eval_str_in(custom_mod.clone(), "(lambda (x) x)")
            .unwrap()
        else {
            panic!("expected bytecode function");
        };
        assert_eq!(custom_fn.module(), &custom_mod);
    }

    #[test]
    fn bytecode_function_defines_and_looks_up_in_its_own_module() {
        let mut vm = Vm::new();
        let math_mod = ModuleId::internal("math");

        vm.eval_str_in(math_mod.clone(), "(define base 100)")
            .unwrap();
        let setter_and_adder = vm
            .eval_str_in(
                math_mod.clone(),
                "(lambda (v) (define saved (+ base v)) saved)",
            )
            .unwrap();

        assert_eq!(
            vm.eval(setter_and_adder.clone(), [Val::Int(23)]),
            Ok(Val::Int(123))
        );
        assert_eq!(
            vm.module(&math_mod).unwrap().get(&Symbol::from("saved")),
            Some(&Val::Int(123))
        );
        assert_eq!(
            vm.module(&ModuleId::internal("user"))
                .unwrap()
                .get(&Symbol::from("saved")),
            None
        );

        // Also verify calling from another module preserves both modules' scopes.
        vm.module_or_init(&ModuleId::internal("user"))
            .insert("run-math", setter_and_adder);
        assert_eq!(
            vm.eval_str("(define base 1) (define after (run-math 5)) (+ base after)"),
            Ok(Val::Int(106))
        );
        assert_eq!(
            vm.module(&math_mod).unwrap().get(&Symbol::from("saved")),
            Some(&Val::Int(105))
        );
        assert_eq!(
            vm.module(&ModuleId::internal("user"))
                .unwrap()
                .get(&Symbol::from("after")),
            Some(&Val::Int(105))
        );
    }

    #[test]
    fn lookup_falls_back_to_global_module_and_allows_module_shadowing() {
        let mut vm = Vm::new();
        assert!(matches!(
            vm.module(&ModuleId::GLOBAL)
                .unwrap()
                .get(&Symbol::from("+")),
            Some(Val::NativeFunction(_))
        ));
        assert_eq!(
            vm.module(&ModuleId::internal("user"))
                .unwrap()
                .get(&Symbol::from("+")),
            None
        );

        vm.eval_str_in(ModuleId::GLOBAL, "(define shared 7)")
            .unwrap();
        assert_eq!(vm.eval_str("(+ shared 3)"), Ok(Val::Int(10)));

        let custom_mod = ModuleId::internal("custom");
        assert_eq!(
            vm.eval_str_in(custom_mod.clone(), "(define shared 100) (+ shared 3)"),
            Ok(Val::Int(103))
        );
        assert_eq!(vm.eval_str("shared"), Ok(Val::Int(7)));
    }

    #[test]
    fn define_in_missing_module_returns_module_not_found_error() {
        use crate::val::function::BytecodeFunction;

        let mut vm = Vm::new();
        let missing_mod = ModuleId::internal("missing");
        let func = Val::BytecodeFunction(BytecodeFunction {
            module: missing_mod.clone(),
            name: None,
            parameters: vec![],
            instructions: [
                Instruction::PushConst(Val::Int(42)),
                Instruction::Define(crate::val::Symbol::from("x")),
                Instruction::PushConst(Val::Nil),
                Instruction::Return,
            ]
            .into(),
        });

        assert_eq!(vm.eval(func, []), Err(VmError::ModuleNotFound(missing_mod)));
    }

    #[test]
    fn import_allows_qualified_lookup_across_modules() {
        let mut vm = Vm::new();
        let math_mod = ModuleId::file("lib/math.tl");
        assert_eq!(
            vm.eval_str_in(math_mod, "(define inc (lambda (x) (+ x 1)))"),
            Ok(Val::Nil)
        );

        assert_eq!(
            vm.eval_str(r#"(import "lib/math.tl" as m) (m.inc 41)"#),
            Ok(Val::Int(42))
        );
    }

    #[test]
    fn eval_str_returns_parse_error_instead_of_panicking() {
        let mut vm = Vm::new();
        assert!(matches!(vm.eval_str(".foo"), Err(VmError::ParseError(_))));
        assert!(matches!(vm.eval_str("(import)"), Err(VmError::ParseError(_))));
    }

    #[test]
    fn lookup_of_uninitialized_define_returns_symbol_not_found() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str("(define (f) g) (f) (define g 42)"),
            Err(VmError::SymbolNotFound {
                module: ModuleId::USER,
                name: Symbol::from("g"),
            })
        );
    }

    #[test]
    fn mark_loaded_sets_module_loaded_state() {
        use crate::val::function::BytecodeFunction;

        let mut vm = Vm::new();
        let file_mod = ModuleId::file("lib/math.tl");
        vm.module_or_init(&file_mod);
        assert!(!vm.module(&file_mod).unwrap().is_loaded());

        let func = Val::BytecodeFunction(BytecodeFunction {
            module: ModuleId::USER,
            name: None,
            parameters: vec![],
            instructions: [
                Instruction::MarkLoaded(file_mod.clone()),
                Instruction::PushConst(Val::Nil),
                Instruction::Return,
            ]
            .into(),
        });

        assert_eq!(vm.eval(func, []), Ok(Val::Nil));
        assert!(vm.module(&file_mod).unwrap().is_loaded());
    }

    #[test]
    fn mark_loaded_missing_module_returns_module_not_found_error() {
        use crate::val::function::BytecodeFunction;

        let mut vm = Vm::new();
        let missing_mod = ModuleId::file("lib/missing.tl");
        let func = Val::BytecodeFunction(BytecodeFunction {
            module: ModuleId::USER,
            name: None,
            parameters: vec![],
            instructions: [
                Instruction::MarkLoaded(missing_mod.clone()),
                Instruction::PushConst(Val::Nil),
                Instruction::Return,
            ]
            .into(),
        });

        assert_eq!(vm.eval(func, []), Err(VmError::ModuleNotFound(missing_mod)));
    }

    #[test]
    fn eval_str_in_file_module_does_not_mark_module_loaded() {
        let mut vm = Vm::new();
        let file_mod = ModuleId::file("lib/branch.tl");
        vm.module_or_init(&file_mod);
        assert!(!vm.module(&file_mod).unwrap().is_loaded());

        assert_eq!(
            vm.eval_str_in(file_mod.clone(), "(if #true 1 2)"),
            Ok(Val::Int(1))
        );
        assert!(!vm.module(&file_mod).unwrap().is_loaded());
    }

    #[test]
    fn load_file_evaluates_file_and_marks_module_loaded() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "takolisp_load_file_test_{}.tl",
            std::process::id()
        ));
        std::fs::write(&path, "(define answer 42) (if #true answer 0)").unwrap();

        let mut vm = Vm::new();
        let file_mod = ModuleId::file(&path);
        let res = vm.load_file(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(res, Ok(Val::Int(42)));
        let module = vm.module(&file_mod).unwrap();
        assert!(module.is_loaded());
        assert_eq!(module.get(&"answer".into()), Some(&Val::Int(42)));
    }

    #[test]
    fn load_file_missing_file_returns_io_error() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "takolisp_missing_file_test_{}.tl",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        let mut vm = Vm::new();
        assert_eq!(
            vm.load_file(&path),
            Err(VmError::IoError {
                path,
                kind: std::io::ErrorKind::NotFound,
            })
        );
    }
}
