use std::collections::HashMap;

use crate::{
    Module, ModuleId,
    ast::{Ast, AstKind},
    val::{
        Symbol, Val,
        function::{BytecodeFunction, Instruction},
    },
};

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum CompilerError {
    ModuleNotFound(ModuleId),
    ImportNotFound { module: ModuleId, alias: Symbol },
    ImportNotAtTopLevel,
}

pub struct Compiler<'a> {
    module: ModuleId,
    modules: &'a mut HashMap<ModuleId, Module>,
    parameters: Vec<Symbol>,
    instructions: Vec<Instruction>,
}

impl<'a> Compiler<'a> {
    pub fn new(
        module: ModuleId,
        modules: &'a mut HashMap<ModuleId, Module>,
        parameters: Vec<Symbol>,
    ) -> Compiler<'a> {
        modules
            .entry(module.clone())
            .or_insert_with(|| Module::new(module.clone()));
        Compiler {
            module,
            modules,
            parameters,
            instructions: Vec::new(),
        }
    }

    pub fn for_module(
        module: ModuleId,
        modules: &'a mut HashMap<ModuleId, Module>,
    ) -> Compiler<'a> {
        Self::new(module, modules, Vec::new())
    }

    fn into_bytecode(self, name: Option<Symbol>) -> BytecodeFunction {
        BytecodeFunction {
            module: self.module,
            name,
            parameters: self.parameters,
            instructions: self.instructions.into(),
        }
    }

    pub fn compile_to_instructions(mut self, ast: &Ast) -> Result<Vec<Instruction>, CompilerError> {
        self.prepare_module(ast, true)?;
        self.compile_one(ast, true)?;
        Ok(self.instructions)
    }

    pub fn compile_module(mut self, ast: &Ast) -> Result<Vec<Instruction>, CompilerError> {
        match &self.module {
            ModuleId::File(_) => {
                self.prepare_module(ast, true)?;
                self.compile_one(ast, false)?;
                self.push(Instruction::MarkLoaded(self.module.clone()));
                self.push(Instruction::Return);
                Ok(self.instructions)
            }
            ModuleId::Internal(_) => self.compile_to_instructions(ast),
        }
    }

    fn prepare_module(&mut self, ast: &Ast, is_top_level: bool) -> Result<(), CompilerError> {
        match &ast.kind {
            AstKind::Define { name, value } => {
                let current_module = self
                    .modules
                    .get_mut(&self.module)
                    .ok_or_else(|| CompilerError::ModuleNotFound(self.module.clone()))?;
                if current_module.get(name).is_none() {
                    current_module.insert(name.clone(), Val::Undefined);
                }
                self.prepare_module(value, false)?;
            }
            AstKind::Import { module, alias } => {
                if !is_top_level {
                    return Err(CompilerError::ImportNotAtTopLevel);
                }
                let current_module = self
                    .modules
                    .get_mut(&self.module)
                    .ok_or_else(|| CompilerError::ModuleNotFound(self.module.clone()))?;
                current_module.import(alias.clone(), module.clone());
            }
            AstKind::TopLevel(exprs) => {
                for expr in exprs {
                    self.prepare_module(expr, is_top_level)?;
                }
            }
            AstKind::Begin(exprs) => {
                for expr in exprs {
                    self.prepare_module(expr, false)?;
                }
            }
            AstKind::Eval { func, args } => {
                self.prepare_module(func, false)?;
                for arg in args {
                    self.prepare_module(arg, false)?;
                }
            }
            AstKind::If {
                pred,
                then_branch,
                else_branch,
            } => {
                self.prepare_module(pred, false)?;
                self.prepare_module(then_branch, false)?;
                if let Some(else_branch) = else_branch {
                    self.prepare_module(else_branch, false)?;
                }
            }
            AstKind::Lambda { body, .. } => {
                for expr in body {
                    self.prepare_module(expr, false)?;
                }
            }
            AstKind::Const(_) | AstKind::Lookup { .. } => {}
        }
        Ok(())
    }

    fn push(&mut self, instruction: Instruction) -> usize {
        let idx = self.instructions.len();
        self.instructions.push(instruction);
        idx
    }

    fn compile_one(&mut self, ast: &Ast, tail_call: bool) -> Result<(), CompilerError> {
        match &ast.kind {
            AstKind::Begin(exprs) | AstKind::TopLevel(exprs) => {
                self.compile_begin(exprs.as_slice(), tail_call)?;
                return Ok(());
            }
            AstKind::If {
                pred,
                then_branch,
                else_branch,
            } => {
                self.compile_if(pred, then_branch, else_branch.as_deref(), tail_call)?;
                return Ok(());
            }
            AstKind::Const(val) => self.compile_const(val.clone()),
            AstKind::Eval { func, args } => self.compile_eval(func, args.as_slice())?,
            AstKind::Define { name, value } => {
                self.compile_one(value, false)?;
                self.push(Instruction::Define(name.clone()));
            }
            AstKind::Lookup { module_alias, name } => {
                self.compile_lookup(module_alias.as_ref(), name)?
            }
            AstKind::Lambda {
                name,
                parameters,
                body,
            } => self.compile_lambda(name.clone(), parameters.clone(), body)?,
            AstKind::Import { module, .. } => {
                self.push(Instruction::EnsureLoaded(module.clone()));
                self.compile_const(Val::Nil);
            }
        }
        if tail_call {
            self.push(Instruction::Return);
        }
        Ok(())
    }

    fn compile_eval(&mut self, func: &Ast, args: &[Ast]) -> Result<(), CompilerError> {
        self.compile_one(func, false)?;
        for arg in args {
            self.compile_one(arg, false)?;
        }
        self.push(Instruction::Eval(args.len()));
        Ok(())
    }

    fn compile_lookup(
        &mut self,
        module_alias: Option<&Symbol>,
        ident: &Symbol,
    ) -> Result<(), CompilerError> {
        // 1. Lookup from module
        if let Some(alias) = module_alias {
            let current_module = self
                .modules
                .get(&self.module)
                .ok_or_else(|| CompilerError::ModuleNotFound(self.module.clone()))?;
            let target_module =
                current_module
                    .get_import(alias)
                    .ok_or_else(|| CompilerError::ImportNotFound {
                        module: self.module.clone(),
                        alias: alias.clone(),
                    })?;
            self.push(Instruction::Lookup(target_module.clone(), ident.clone()));
            return Ok(());
        }

        // 2. Function parameter
        if let Some(idx) = self.parameters.iter().rposition(|param| param == ident) {
            self.push(Instruction::LookupLocal(idx));
            return Ok(());
        }

        // 3. Local module lookup
        let current_module = self
            .modules
            .get(&self.module)
            .ok_or_else(|| CompilerError::ModuleNotFound(self.module.clone()))?;
        let target_module = if current_module.get(ident).is_some() {
            self.module.clone()
        } else {
            ModuleId::GLOBAL
        };
        self.push(Instruction::Lookup(target_module, ident.clone()));
        Ok(())
    }

    fn compile_const(&mut self, val: Val) {
        self.push(Instruction::PushConst(val));
    }

    fn compile_begin(&mut self, exprs: &[Ast], tail_call: bool) -> Result<(), CompilerError> {
        match exprs {
            [] => {
                self.push(Instruction::PushConst(Val::Nil));
            }
            [expr] => self.compile_one(expr, tail_call)?,
            [exprs @ .., final_expr] => {
                for expr in exprs {
                    self.compile_one(expr, false)?;
                }
                if !tail_call {
                    self.push(Instruction::Pop(exprs.len()));
                }
                self.compile_one(final_expr, tail_call)?;
            }
        }
        Ok(())
    }

    fn compile_if(
        &mut self,
        pred: &Ast,
        then_branch: &Ast,
        else_branch: Option<&Ast>,
        tail_call: bool,
    ) -> Result<(), CompilerError> {
        // Predicate
        self.compile_one(pred, false)?;
        let then_jump_idx = self.push(Instruction::JumpIfNot(0));

        // Then branch
        self.compile_one(then_branch, tail_call)?;
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
        self.compile_one(else_branch.unwrap_or(&nil_else), tail_call)?;

        // Jump offsets are relative to the instruction after the jump.
        let end = self.instructions.len();
        self.instructions[then_jump_idx] = Instruction::JumpIfNot(else_start - then_jump_idx - 1);
        if !tail_call {
            self.instructions[end_jump_idx] = Instruction::Jump(end - end_jump_idx - 1);
        }
        Ok(())
    }

    fn compile_lambda(
        &mut self,
        name: Option<Symbol>,
        parameters: Vec<Symbol>,
        body: &[Ast],
    ) -> Result<(), CompilerError> {
        let mut sub_compiler = Compiler::new(self.module.clone(), self.modules, parameters);
        sub_compiler.compile_begin(body, true)?;
        let lambda_val = Val::BytecodeFunction(sub_compiler.into_bytecode(name));
        self.compile_const(lambda_val);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_parameter_emits_lookup_local() {
        let ast = Ast::parse_in(ModuleId::USER, "b").unwrap();
        let instructions = Compiler::new(
            ModuleId::USER,
            &mut HashMap::new(),
            vec!["a".into(), "b".into()],
        )
        .compile_to_instructions(&ast)
        .unwrap();
        assert_eq!(
            instructions,
            vec![Instruction::LookupLocal(1), Instruction::Return]
        );
    }

    #[test]
    fn lookup_non_parameter_emits_lookup_global() {
        let ast = Ast::parse_in(ModuleId::USER, "+").unwrap();
        let instructions = Compiler::new(
            ModuleId::USER,
            &mut HashMap::new(),
            vec!["a".into(), "b".into()],
        )
        .compile_to_instructions(&ast)
        .unwrap();
        assert_eq!(
            instructions,
            vec![
                Instruction::Lookup(ModuleId::GLOBAL, "+".into()),
                Instruction::Return
            ]
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
        let instructions = Compiler::new(
            ModuleId::USER,
            &mut HashMap::new(),
            vec!["a".into(), "b".into(), "a".into()],
        )
        .compile_to_instructions(&expr)
        .unwrap();
        assert_eq!(
            instructions,
            vec![Instruction::LookupLocal(2), Instruction::Return]
        );
    }

    #[test]
    fn constant_at_tail_emits_push_and_return() {
        let expr = Ast::parse_in(ModuleId::USER, "42").unwrap();
        let instructions = Compiler::for_module(ModuleId::USER, &mut HashMap::new())
            .compile_to_instructions(&expr)
            .unwrap();
        assert_eq!(
            instructions,
            vec![Instruction::PushConst(Val::Int(42)), Instruction::Return]
        );
    }

    #[test]
    fn empty_begin_emits_nil_constant() {
        let expr = ast(AstKind::Begin(vec![]));
        let instructions = Compiler::for_module(ModuleId::USER, &mut HashMap::new())
            .compile_to_instructions(&expr)
            .unwrap();
        assert_eq!(instructions, vec![Instruction::PushConst(Val::Nil)]);
    }

    #[test]
    fn non_tail_begin_pops_intermediate_values() {
        let expr = ast(AstKind::Eval {
            func: Box::new(ast(AstKind::Lookup {
                module_alias: None,
                name: "+".into(),
            })),
            args: vec![ast(AstKind::Begin(vec![
                ast(AstKind::Const(Val::Int(1))),
                ast(AstKind::Const(Val::Int(2))),
                ast(AstKind::Const(Val::Int(3))),
            ]))],
        });
        let instructions = Compiler::for_module(ModuleId::USER, &mut HashMap::new())
            .compile_to_instructions(&expr)
            .unwrap();
        assert_eq!(
            instructions,
            vec![
                Instruction::Lookup(ModuleId::GLOBAL, "+".into()),
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
        let instructions = Compiler::for_module(ModuleId::USER, &mut HashMap::new())
            .compile_to_instructions(&expr)
            .unwrap();
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
        let instructions = Compiler::for_module(ModuleId::USER, &mut HashMap::new())
            .compile_to_instructions(&expr)
            .unwrap();
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
        let instructions = Compiler::for_module(ModuleId::USER, &mut HashMap::new())
            .compile_to_instructions(&expr)
            .unwrap();
        assert_eq!(
            instructions,
            vec![
                Instruction::Lookup(ModuleId::GLOBAL, "+".into()),
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
        let instructions = Compiler::for_module(ModuleId::USER, &mut HashMap::new())
            .compile_to_instructions(&expr)
            .unwrap();
        assert_eq!(
            instructions,
            vec![
                Instruction::Lookup(ModuleId::GLOBAL, "+".into()),
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
        let instructions = Compiler::for_module(ModuleId::USER, &mut HashMap::new())
            .compile_to_instructions(&expr)
            .unwrap();
        assert_eq!(
            instructions,
            vec![
                Instruction::PushConst(Val::BytecodeFunction(BytecodeFunction {
                    module: ModuleId::internal("user"),
                    name: None,
                    parameters: vec!["x".into(), "y".into()],
                    instructions: vec![
                        Instruction::Lookup(ModuleId::GLOBAL, "+".into()),
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
        let instructions = Compiler::for_module(module.clone(), &mut HashMap::new())
            .compile_to_instructions(&expr)
            .unwrap();
        assert_eq!(
            instructions,
            vec![
                Instruction::PushConst(Val::BytecodeFunction(BytecodeFunction {
                    module: module.clone(),
                    name: None,
                    parameters: vec!["x".into()],
                    instructions: vec![
                        Instruction::PushConst(Val::BytecodeFunction(BytecodeFunction {
                            module: module.clone(),
                            name: None,
                            parameters: vec!["y".into()],
                            instructions: vec![
                                Instruction::Lookup(ModuleId::GLOBAL, "+".into()),
                                Instruction::Lookup(ModuleId::GLOBAL, "x".into()),
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
        let instructions = Compiler::for_module(ModuleId::USER, &mut HashMap::new())
            .compile_to_instructions(&expr)
            .unwrap();
        assert_eq!(
            instructions,
            vec![
                Instruction::PushConst(Val::Int(1)),
                Instruction::Define("x".into()),
                Instruction::Return,
            ]
        );
    }

    #[test]
    fn compiler_creates_module_if_missing() {
        let mut modules = HashMap::new();
        let module_id = ModuleId::file("examples/new_mod.tl");
        assert!(!modules.contains_key(&module_id));

        Compiler::for_module(module_id.clone(), &mut modules);
        assert!(modules.contains_key(&module_id));

        modules
            .get_mut(&module_id)
            .unwrap()
            .insert("existing", Val::Int(7));
        Compiler::for_module(module_id.clone(), &mut modules);
        assert_eq!(
            modules.get(&module_id).unwrap().get(&"existing".into()),
            Some(&Val::Int(7))
        );
    }

    #[test]
    fn import_sets_alias_on_current_module_and_returns_nil() {
        let mut modules = HashMap::new();
        let current = ModuleId::internal("user");
        let expr = Ast::parse_in(current.clone(), r#"(import "path/to/file.tl" as f)"#).unwrap();
        let instructions = Compiler::for_module(current.clone(), &mut modules)
            .compile_to_instructions(&expr)
            .unwrap();

        assert_eq!(
            modules.get(&current).unwrap().get_import(&"f".into()),
            Some(&ModuleId::file("path/to/file.tl"))
        );
        assert_eq!(
            instructions,
            vec![
                Instruction::EnsureLoaded(ModuleId::file("path/to/file.tl")),
                Instruction::PushConst(Val::Nil),
                Instruction::Return,
            ]
        );
    }

    #[test]
    fn import_returns_module_not_found_if_current_module_missing() {
        let mut modules = HashMap::new();
        let current = ModuleId::internal("user");
        let expr = Ast::parse_in(current.clone(), r#"(import "path/to/file.tl" as f)"#).unwrap();
        let compiler = Compiler::for_module(current.clone(), &mut modules);
        compiler.modules.remove(&current);

        assert_eq!(
            compiler.compile_to_instructions(&expr),
            Err(CompilerError::ModuleNotFound(current))
        );
    }

    #[test]
    fn initial_pass_sets_undefined_defines_without_overwriting_existing() {
        let mut modules = HashMap::new();
        let current = ModuleId::USER;
        let mut user_mod = Module::new(current.clone());
        user_mod.insert("existing", Val::Int(99));
        modules.insert(current.clone(), user_mod);

        let expr = Ast::parse_in(
            current.clone(),
            "(define existing 1) (define fresh 2) (lambda () (define nested 3))",
        )
        .unwrap();
        Compiler::for_module(current.clone(), &mut modules)
            .compile_to_instructions(&expr)
            .unwrap();

        let user_mod = modules.get(&current).unwrap();
        assert_eq!(user_mod.get(&"existing".into()), Some(&Val::Int(99)));
        assert_eq!(user_mod.get(&"fresh".into()), Some(&Val::Undefined));
        assert_eq!(user_mod.get(&"nested".into()), Some(&Val::Undefined));
    }

    #[test]
    fn lookup_uses_current_module_for_defined_symbols_including_forward_refs() {
        let mut modules = HashMap::new();
        let current = ModuleId::file("examples/app.tl");
        let expr =
            Ast::parse_in(current.clone(), "(define f (lambda () g)) (define g 42) g").unwrap();
        let instructions = Compiler::for_module(current.clone(), &mut modules)
            .compile_to_instructions(&expr)
            .unwrap();

        assert_eq!(instructions.last(), Some(&Instruction::Return));
        assert!(instructions.contains(&Instruction::Lookup(current.clone(), "g".into())));
        let Val::BytecodeFunction(f) = (match &instructions[0] {
            Instruction::PushConst(val) => val,
            other => panic!("expected PushConst, got {other:?}"),
        }) else {
            panic!("expected BytecodeFunction");
        };
        assert_eq!(
            f.instructions.as_ref(),
            &[
                Instruction::Lookup(current, "g".into()),
                Instruction::Return
            ]
        );
    }

    #[test]
    fn lookup_resolves_module_alias_via_imports() {
        let mut modules = HashMap::new();
        let current = ModuleId::USER;
        let expr = Ast::parse_in(current.clone(), r#"(import "lib/math.tl" as m) m.add"#).unwrap();
        let instructions = Compiler::for_module(current, &mut modules)
            .compile_to_instructions(&expr)
            .unwrap();

        assert_eq!(
            instructions,
            vec![
                Instruction::EnsureLoaded(ModuleId::file("lib/math.tl")),
                Instruction::PushConst(Val::Nil),
                Instruction::Lookup(ModuleId::file("lib/math.tl"), "add".into()),
                Instruction::Return,
            ]
        );
    }

    #[test]
    fn lookup_with_unknown_module_alias_returns_error() {
        let mut modules = HashMap::new();
        let current = ModuleId::USER;
        let expr = Ast::parse_in(current.clone(), "unknown.fn").unwrap();
        let res =
            Compiler::for_module(current.clone(), &mut modules).compile_to_instructions(&expr);

        assert_eq!(
            res,
            Err(CompilerError::ImportNotFound {
                module: current,
                alias: "unknown".into(),
            })
        );
    }

    #[test]
    fn import_after_lookup_is_resolved_by_initial_pass() {
        let mut modules = HashMap::new();
        let current = ModuleId::USER;
        let expr = Ast::parse_in(
            current.clone(),
            r#"(define f (lambda () m.add)) (import "lib/math.tl" as m)"#,
        )
        .unwrap();
        let instructions = Compiler::for_module(current, &mut modules)
            .compile_to_instructions(&expr)
            .unwrap();

        let Val::BytecodeFunction(f) = (match &instructions[0] {
            Instruction::PushConst(val) => val,
            other => panic!("expected PushConst, got {other:?}"),
        }) else {
            panic!("expected BytecodeFunction");
        };
        assert_eq!(
            f.instructions.as_ref(),
            &[
                Instruction::Lookup(ModuleId::file("lib/math.tl"), "add".into()),
                Instruction::Return,
            ]
        );
    }

    #[test]
    fn non_top_level_import_returns_error() {
        for source in [
            r#"(if #true (import "lib/math.tl"))"#,
            r#"(lambda () (import "lib/math.tl"))"#,
            r#"(define x (import "lib/math.tl"))"#,
            r#"(+ (import "lib/math.tl"))"#,
        ] {
            let mut modules = HashMap::new();
            let expr = Ast::parse_in(ModuleId::USER, source).unwrap();
            let res =
                Compiler::for_module(ModuleId::USER, &mut modules).compile_to_instructions(&expr);
            assert_eq!(res, Err(CompilerError::ImportNotAtTopLevel), "{source}");
        }

        let begin_import = ast(AstKind::Begin(vec![ast(AstKind::Import {
            module: ModuleId::file("lib/math.tl"),
            alias: "math".into(),
        })]));
        let mut modules = HashMap::new();
        let res = Compiler::for_module(ModuleId::USER, &mut modules)
            .compile_to_instructions(&begin_import);
        assert_eq!(res, Err(CompilerError::ImportNotAtTopLevel));
    }

    #[test]
    fn compile_module_emits_mark_loaded_for_file_modules_only() {
        let file_mod = ModuleId::file("lib/math.tl");
        let file_expr = Ast::parse_in(file_mod.clone(), "(if #true 1 2)").unwrap();
        let file_instructions = Compiler::for_module(file_mod.clone(), &mut HashMap::new())
            .compile_module(&file_expr)
            .unwrap();
        assert_eq!(
            file_instructions,
            vec![
                Instruction::PushConst(Val::Bool(true)),
                Instruction::JumpIfNot(2),
                Instruction::PushConst(Val::Int(1)),
                Instruction::Jump(1),
                Instruction::PushConst(Val::Int(2)),
                Instruction::MarkLoaded(file_mod),
                Instruction::Return,
            ]
        );

        let internal_expr = Ast::parse_in(ModuleId::USER, "(if #true 1 2)").unwrap();
        let internal_instructions = Compiler::for_module(ModuleId::USER, &mut HashMap::new())
            .compile_module(&internal_expr)
            .unwrap();
        assert!(
            !internal_instructions
                .iter()
                .any(|instr| matches!(instr, Instruction::MarkLoaded(_)))
        );
    }
}
