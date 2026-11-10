use crate::{
    ast::{Ast, AstKind},
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
            [] => self.push(Instruction::PushConst(Val::Nil)),
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

    fn push(&mut self, instruction: Instruction) {
        self.instructions.push(instruction);
    }
}
