use crate::{
    ModuleId,
    ast::{Ast, AstKind},
    val::{
        Symbol, Val,
        function::{BytecodeFunction, Instruction},
    },
};

pub struct Compiler {
    module: ModuleId,
    parameters: Vec<Symbol>,
    instructions: Vec<Instruction>,
}

impl Compiler {
    pub fn new(module: ModuleId, parameters: Vec<Symbol>) -> Compiler {
        Compiler {
            module,
            parameters,
            instructions: Vec::new(),
        }
    }

    pub fn for_module(module: ModuleId) -> Compiler {
        Self::new(module, Vec::new())
    }

    fn into_bytecode(self, name: Option<Symbol>) -> BytecodeFunction {
        BytecodeFunction {
            module: self.module,
            name,
            parameters: self.parameters,
            instructions: self.instructions.into(),
        }
    }

    pub fn compile_to_instructions(mut self, ast: &Ast) -> Vec<Instruction> {
        self.compile_one(ast, true);
        self.instructions
    }

    fn push(&mut self, instruction: Instruction) -> usize {
        let idx = self.instructions.len();
        self.instructions.push(instruction);
        idx
    }

    fn compile_one(&mut self, ast: &Ast, tail_call: bool) {
        match &ast.kind {
            AstKind::Begin(exprs) => {
                self.compile_begin(exprs.as_slice(), tail_call);
                return;
            }
            AstKind::If {
                pred,
                then_branch,
                else_branch,
            } => {
                self.compile_if(pred, then_branch, else_branch.as_deref(), tail_call);
                return;
            }
            AstKind::Const(val) => self.compile_const(val.clone()),
            AstKind::Eval { func, args } => self.compile_eval(func, args.as_slice()),
            AstKind::Define { name, value } => {
                self.compile_one(value, false);
                self.push(Instruction::Define(name.clone()));
            }
            AstKind::Lookup { module: _, name } => self.compile_lookup(name),
            AstKind::Lambda {
                name,
                parameters,
                body,
            } => self.compile_lambda(name.clone(), parameters.clone(), body),
        }
        if tail_call {
            self.push(Instruction::Return);
        }
    }

    fn compile_eval(&mut self, func: &Ast, args: &[Ast]) {
        self.compile_one(func, false);
        for arg in args {
            self.compile_one(arg, false);
        }
        self.push(Instruction::Eval(args.len()));
    }

    fn compile_lookup(&mut self, ident: &Symbol) {
        let param_position = self.parameters.iter().rposition(|param| param == ident);
        let instruction = match param_position {
            Some(idx) => Instruction::LookupLocal(idx),
            None => Instruction::LookupGlobal(ident.clone()),
        };
        self.push(instruction);
    }

    fn compile_const(&mut self, val: Val) {
        self.push(Instruction::PushConst(val));
    }

    fn compile_begin(&mut self, exprs: &[Ast], tail_call: bool) {
        match exprs {
            [] => {
                self.push(Instruction::PushConst(Val::Nil));
            }
            [expr] => self.compile_one(expr, tail_call),
            [exprs @ .., final_expr] => {
                for expr in exprs {
                    self.compile_one(expr, false);
                }
                if !tail_call {
                    self.push(Instruction::Pop(exprs.len()));
                }
                self.compile_one(final_expr, tail_call);
            }
        }
    }

    fn compile_if(
        &mut self,
        pred: &Ast,
        then_branch: &Ast,
        else_branch: Option<&Ast>,
        tail_call: bool,
    ) {
        // Predicate
        self.compile_one(pred, false);
        let then_jump_idx = self.push(Instruction::JumpIfNot(0));

        // Then branch
        self.compile_one(then_branch, tail_call);
        let end_jump_idx = self.instructions.len();
        if !tail_call {
            self.push(Instruction::Jump(0));
        }
        let else_start = self.instructions.len();

        // Else Branch
        let nil_else = Ast {
            kind: AstKind::Const(Val::Nil),
            span: (0..0).into(),
        };
        self.compile_one(else_branch.unwrap_or(&nil_else), tail_call);

        // Jump offsets are relative to the instruction after the jump.
        let end = self.instructions.len();
        self.instructions[then_jump_idx] = Instruction::JumpIfNot(else_start - then_jump_idx - 1);
        if !tail_call {
            self.instructions[end_jump_idx] = Instruction::Jump(end - end_jump_idx - 1);
        }
    }

    fn compile_lambda(&mut self, name: Option<Symbol>, parameters: Vec<Symbol>, body: &[Ast]) {
        let mut sub_compiler = Compiler::new(self.module.clone(), parameters);
        sub_compiler.compile_begin(body, true);
        let lambda_val = Val::BytecodeFunction(sub_compiler.into_bytecode(name));
        self.compile_const(lambda_val);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_parameter_emits_lookup_local() {
        let ast = Ast::parse_in(ModuleId::USER, "b").unwrap();
        let instructions = Compiler::new(ModuleId::USER, vec!["a".into(), "b".into()])
            .compile_to_instructions(&ast);
        assert_eq!(
            instructions,
            vec![Instruction::LookupLocal(1), Instruction::Return]
        );
    }

    #[test]
    fn lookup_non_parameter_emits_lookup_global() {
        let ast = Ast::parse_in(ModuleId::USER, "+").unwrap();
        let instructions = Compiler::new(ModuleId::USER, vec!["a".into(), "b".into()])
            .compile_to_instructions(&ast);
        assert_eq!(
            instructions,
            vec![Instruction::LookupGlobal("+".into()), Instruction::Return]
        );
    }

    fn ast(kind: AstKind) -> Ast {
        Ast {
            kind,
            span: (0..0).into(),
        }
    }

    #[test]
    fn duplicate_parameter_lookup_binds_to_innermost_local() {
        let expr = Ast::parse_in(ModuleId::USER, "a").unwrap();
        let instructions = Compiler::new(ModuleId::USER, vec!["a".into(), "b".into(), "a".into()])
            .compile_to_instructions(&expr);
        assert_eq!(
            instructions,
            vec![Instruction::LookupLocal(2), Instruction::Return]
        );
    }

    #[test]
    fn constant_at_tail_emits_push_and_return() {
        let expr = Ast::parse_in(ModuleId::USER, "42").unwrap();
        let instructions = Compiler::for_module(ModuleId::USER).compile_to_instructions(&expr);
        assert_eq!(
            instructions,
            vec![Instruction::PushConst(Val::Int(42)), Instruction::Return]
        );
    }

    #[test]
    fn empty_begin_emits_nil_constant() {
        let expr = ast(AstKind::Begin(vec![]));
        let instructions = Compiler::for_module(ModuleId::USER).compile_to_instructions(&expr);
        assert_eq!(instructions, vec![Instruction::PushConst(Val::Nil)]);
    }

    #[test]
    fn non_tail_begin_pops_intermediate_values() {
        let expr = ast(AstKind::Eval {
            func: Box::new(ast(AstKind::Lookup {
                module: ModuleId::USER,
                name: "+".into(),
            })),
            args: vec![ast(AstKind::Begin(vec![
                ast(AstKind::Const(Val::Int(1))),
                ast(AstKind::Const(Val::Int(2))),
                ast(AstKind::Const(Val::Int(3))),
            ]))],
        });
        let instructions = Compiler::for_module(ModuleId::USER).compile_to_instructions(&expr);
        assert_eq!(
            instructions,
            vec![
                Instruction::LookupGlobal("+".into()),
                Instruction::PushConst(Val::Int(1)),
                Instruction::PushConst(Val::Int(2)),
                Instruction::Pop(2),
                Instruction::PushConst(Val::Int(3)),
                Instruction::Eval(1),
                Instruction::Return,
            ]
        );
    }

    #[test]
    fn if_without_else_defaults_else_branch_to_nil() {
        let expr = Ast::parse_in(ModuleId::USER, "(if #true 1)").unwrap();
        let instructions = Compiler::for_module(ModuleId::USER).compile_to_instructions(&expr);
        assert_eq!(
            instructions,
            vec![
                Instruction::PushConst(Val::Bool(true)),
                Instruction::JumpIfNot(2),
                Instruction::PushConst(Val::Int(1)),
                Instruction::Return,
                Instruction::PushConst(Val::Nil),
                Instruction::Return,
            ]
        );
    }

    #[test]
    fn tail_if_emits_return_in_both_branches_without_trailing_jump() {
        let expr = Ast::parse_in(ModuleId::USER, "(if #true 1 2)").unwrap();
        let instructions = Compiler::for_module(ModuleId::USER).compile_to_instructions(&expr);
        assert_eq!(
            instructions,
            vec![
                Instruction::PushConst(Val::Bool(true)),
                Instruction::JumpIfNot(2),
                Instruction::PushConst(Val::Int(1)),
                Instruction::Return,
                Instruction::PushConst(Val::Int(2)),
                Instruction::Return,
            ]
        );
    }

    #[test]
    fn non_tail_if_jumps_over_else_branch_after_then() {
        let expr = Ast::parse_in(ModuleId::USER, "(+ (if #true 1 2))").unwrap();
        let instructions = Compiler::for_module(ModuleId::USER).compile_to_instructions(&expr);
        assert_eq!(
            instructions,
            vec![
                Instruction::LookupGlobal("+".into()),
                Instruction::PushConst(Val::Bool(true)),
                Instruction::JumpIfNot(2),
                Instruction::PushConst(Val::Int(1)),
                Instruction::Jump(1),
                Instruction::PushConst(Val::Int(2)),
                Instruction::Eval(1),
                Instruction::Return,
            ]
        );
    }

    #[test]
    fn call_emits_func_then_args_then_eval() {
        let expr = Ast::parse_in(ModuleId::USER, "(+ 1 2)").unwrap();
        let instructions = Compiler::for_module(ModuleId::USER).compile_to_instructions(&expr);
        assert_eq!(
            instructions,
            vec![
                Instruction::LookupGlobal("+".into()),
                Instruction::PushConst(Val::Int(1)),
                Instruction::PushConst(Val::Int(2)),
                Instruction::Eval(2),
                Instruction::Return,
            ]
        );
    }

    #[test]
    fn lambda_compiles_body_with_its_own_parameters() {
        let expr = Ast::parse_in(ModuleId::USER, "(lambda (x y) (+ x y))").unwrap();
        let instructions = Compiler::for_module(ModuleId::USER).compile_to_instructions(&expr);
        assert_eq!(
            instructions,
            vec![
                Instruction::PushConst(Val::BytecodeFunction(BytecodeFunction {
                    module: ModuleId::internal("user"),
                    name: None,
                    parameters: vec!["x".into(), "y".into()],
                    instructions: vec![
                        Instruction::LookupGlobal("+".into()),
                        Instruction::LookupLocal(0),
                        Instruction::LookupLocal(1),
                        Instruction::Eval(2),
                        Instruction::Return,
                    ]
                    .into(),
                })),
                Instruction::Return,
            ]
        );
    }

    #[test]
    fn lambda_inherits_module_from_enclosing_compiler() {
        let expr = Ast::parse_in(ModuleId::USER, "(lambda (x) (lambda (y) (+ x y)))").unwrap();
        let module = ModuleId::file("examples/math.tl");
        let instructions = Compiler::for_module(module.clone()).compile_to_instructions(&expr);
        assert_eq!(
            instructions,
            vec![
                Instruction::PushConst(Val::BytecodeFunction(BytecodeFunction {
                    module: module.clone(),
                    name: None,
                    parameters: vec!["x".into()],
                    instructions: vec![
                        Instruction::PushConst(Val::BytecodeFunction(BytecodeFunction {
                            module,
                            name: None,
                            parameters: vec!["y".into()],
                            instructions: vec![
                                Instruction::LookupGlobal("+".into()),
                                Instruction::LookupGlobal("x".into()),
                                Instruction::LookupLocal(0),
                                Instruction::Eval(2),
                                Instruction::Return,
                            ]
                            .into(),
                        })),
                        Instruction::Return,
                    ]
                    .into(),
                })),
                Instruction::Return,
            ]
        );
    }

    #[test]
    fn define_emits_define_instruction() {
        let expr = ast(AstKind::Define {
            name: "x".into(),
            value: Box::new(ast(AstKind::Const(Val::Int(1)))),
        });
        let instructions = Compiler::for_module(ModuleId::USER).compile_to_instructions(&expr);
        assert_eq!(
            instructions,
            vec![
                Instruction::PushConst(Val::Int(1)),
                Instruction::Define("x".into()),
                Instruction::Return,
            ]
        );
    }
}
