use crate::{
    ast::{Ast, AstKind, IfAst},
    val::{Instruction, Val},
};

#[derive(Default)]
pub struct Compiler {
    instructions: Vec<Instruction>,
}

impl Compiler {
    pub fn compile(mut self, ast: &Ast) -> Vec<Instruction> {
        self.compile_one(ast, true);
        self.instructions
    }

    fn compile_one(&mut self, ast: &Ast, tail_call: bool) {
        match &ast.kind {
            AstKind::Const(val) => {
                self.push(Instruction::PushConst(val.clone()));
                if tail_call {
                    self.push(Instruction::Return);
                }
            }
            AstKind::Eval { func, args } => {
                self.compile_eval(func, args.as_slice());
                if tail_call {
                    self.push(Instruction::Return);
                }
            }
            AstKind::Lookup(ident) => {
                self.push(Instruction::Lookup(ident.clone()));
                if tail_call {
                    self.push(Instruction::Return);
                }
            }
            AstKind::Begin(exprs) => self.compile_begin(exprs.as_slice(), tail_call),
            AstKind::If(if_ast) => self.compile_if(if_ast, tail_call),
        }
    }

    fn compile_eval(&mut self, func: &Ast, args: &[Ast]) {
        self.compile_one(func, false);
        for arg in args {
            self.compile_one(arg, false);
        }
        self.push(Instruction::Eval(args.len()));
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

    fn compile_if(&mut self, if_ast: &IfAst, tail_call: bool) {
        // Predicate
        self.compile_one(&if_ast.pred, false);
        let then_jump_idx = self.push(Instruction::JumpIfNot(0));

        // Then branch
        self.compile_one(&if_ast.then_branch, tail_call);
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
        let else_branch = &if_ast
            .else_branch
            .as_ref()
            .map(Box::as_ref)
            .unwrap_or(&nil_else);
        self.compile_one(else_branch, tail_call);

        // Jump offsets are relative to the instruction after the jump.
        let end = self.instructions.len();
        self.instructions[then_jump_idx] = Instruction::JumpIfNot(else_start - then_jump_idx - 1);
        if !tail_call {
            self.instructions[end_jump_idx] = Instruction::Jump(end - end_jump_idx - 1);
        }
    }

    fn push(&mut self, instruction: Instruction) -> usize {
        let idx = self.instructions.len();
        self.instructions.push(instruction);
        idx
    }
}
