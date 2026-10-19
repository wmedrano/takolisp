//! Abstract syntax trees for takolisp expressions.

use bumpalo::Bump;

pub use crate::ast::builder::{AstError, AstErrorKind, AstErrors, FormErrorKind};
use crate::{
    Module,
    ast::{builder::AstBuilder, compiler::CompilerContext},
    error::CompilerError,
    heap::{Heap, HeapHandle},
    reader::Reader,
    source_map::SourceId,
    symbol::Symbol,
    tokenizer::Span,
    val::Val,
    vm::Vm,
};

mod builder;
mod compiler;

#[cfg(test)]
use crate::ModuleSource;

/// An AST (Abstract Syntax Tree) node.
#[derive(Clone, Debug, PartialEq)]
pub struct Ast<'a> {
    /// The actual node variant.
    pub kind: AstKind<'a>,
    /// The location of the `Ast` in the source.
    pub span: Span,
}

/// The type of `Ast` node.
///
/// Variants are grouped in this order: atoms, then binding and assignment, then abstraction and
/// application, then control flow. Matching code, such as the compiler and the builder, follows
/// the same order.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum AstKind<'a> {
    // Atoms, which have no sub-expressions.
    /// An expression that evaluates to a constant.
    Constant(Val),
    /// An expression that looks up a symbol, optionally qualified by a namespace.
    /// The final slash separates the namespace from the symbol, so `x/y/z`
    /// looks up `z` in namespace `x/y`.
    Lookup {
        namespace: Option<Symbol>,
        symbol: Symbol,
    },

    /// Binding and assignment, which associate a symbol with a value
    /// or a module.
    /// Evaluates `value` and defines it as a global `symbol`.
    Define { symbol: Symbol, value: &'a Ast<'a> },
    /// Loads the module at `path` and binds it to `alias`.
    Import { path: &'a str, alias: Symbol },
    /// Evaluates each binding in order and then the body.
    Let {
        bindings: &'a [LetBinding<'a>],
        body: &'a Ast<'a>,
    },
    /// Sets the value at `symbol` to `value`.
    Set { symbol: Symbol, value: &'a Ast<'a> },

    // Abstraction and application.
    /// Creates a function with the specified arguments and body.
    Lambda {
        args: &'a [Symbol],
        body: &'a Ast<'a>,
    },
    /// Calls `function` with `args`.
    FunctionCall {
        function: &'a Ast<'a>,
        args: &'a [Ast<'a>],
    },

    // Control flow.
    /// A conditional expression.
    If {
        /// The expression used to select a branch.
        condition: &'a Ast<'a>,
        /// The expression evaluated when the condition is truthy.
        then_branch: &'a Ast<'a>,
        /// The expression evaluated when the condition is falsy.
        else_branch: &'a Ast<'a>,
    },
    /// A sequence of expressions whose value is the value of the last expression.
    Begin(&'a [Ast<'a>]),
    /// Evaluates `value` and returns it from the current function.
    Return(&'a Ast<'a>),
}

/// One `symbol` and initializer pair of an [`AstKind::Let`] expression.
#[derive(Clone, Debug, PartialEq)]
pub struct LetBinding<'a> {
    pub symbol: Symbol,
    pub value: &'a Ast<'a>,
}

impl<'a> Default for AstKind<'a> {
    fn default() -> Self {
        Self::Constant(Val::Undefined)
    }
}

impl<'a> Ast<'a> {
    /// Parse the source forms into an AST.
    ///
    /// Multiple top-level forms are wrapped in a [`AstKind::Begin`] node, and
    /// returns all parsing and form errors encountered in an [`AstErrors`] value.
    pub fn parse<'b>(
        source_id: SourceId,
        source: &str,
        bump: &'a Bump,
        heap: &'b mut Heap,
    ) -> Result<Ast<'a>, AstErrors> {
        let mut reader = Reader::new(source);
        let mut visitor = AstBuilder::new(source_id, bump, heap);
        while let Some(read_result) = reader.read(&mut visitor) {
            if let Err(err) = read_result {
                std::hint::cold_path();
                visitor.push_error(AstErrorKind::Read(err.kind), err.token_span);
            }
        }
        visitor.into_ast(0..source.len())
    }

    /// Compile an AST into a function value using the supplied module.
    ///
    /// Global values are inlined as constants at compile time.
    pub fn compile(&self, vm: &mut Vm, module: HeapHandle<Module>) -> Result<Val, CompilerError> {
        let global_module = vm.global_module();
        let heap = vm.heap_mut();
        let func = CompilerContext::new(heap, module, global_module).compile(self)?;
        let handle = heap.bytecode_functions.insert(func);
        Ok(handle.into())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        ast::builder::{AstError, FormErrorKind},
        function::{Arity, BytecodeFunction, CaptureSource, Instruction},
        reader::ReadErrorKind,
    };

    use super::*;

    fn must_parse<'a>(source: &str, bump: &'a Bump, heap: &mut Heap) -> Ast<'a> {
        Ast::parse(SourceId::new_unique(), source, bump, heap).expect("expected valid AST")
    }

    fn must_error(source: &str) -> AstErrors {
        let bump = Bump::new();
        Ast::parse(SourceId::new_unique(), source, &bump, &mut Heap::default())
            .expect_err("expected errors")
    }

    fn compile<'a>(
        ast: &Ast<'_>,
        vm: &'a mut Vm,
    ) -> Result<&'a BytecodeFunction, crate::error::CompilerError> {
        compile_in_module(ast, vm, vm.global_module())
    }

    fn compile_in_module<'a>(
        ast: &Ast<'_>,
        vm: &'a mut Vm,
        module: HeapHandle<Module>,
    ) -> Result<&'a BytecodeFunction, crate::error::CompilerError> {
        let Val::BytecodeFunction(handle) = ast.compile(vm, module)? else {
            unreachable!("compiling an AST must produce a bytecode function")
        };
        Ok(vm.heap().bytecode_functions.get(handle).unwrap())
    }

    #[test]
    fn empty_source_reports_no_expression() {
        assert_eq!(
            must_error("").as_slice(),
            &[AstError {
                kind: AstErrorKind::Form(FormErrorKind::NoExpression),
                span: 0..0,
            }],
        );
    }

    #[test]
    fn whitespace_only_source_reports_no_expression() {
        assert_eq!(
            must_error(" \t\n ").as_slice(),
            &[AstError {
                kind: AstErrorKind::Form(FormErrorKind::NoExpression),
                span: 0..4,
            }],
        );
    }

    #[test]
    fn define_compiles_to_define_instruction() {
        let bump = Bump::new();
        let mut vm = Vm::default();
        let ast = must_parse("(define x 42)", &bump, vm.heap_mut());

        assert_eq!(
            compile(&ast, &mut vm).unwrap().bytecode.as_ref(),
            &[
                Instruction::PushConst(Val::Int(42)),
                Instruction::Define(crate::module::ModuleSlot::new(0)),
                Instruction::Return,
            ],
        );
    }

    #[test]
    fn lookup_compiles_to_module_slot_when_symbol_is_available() {
        let bump = Bump::new();
        let mut vm = Vm::default();
        let symbol = Symbol::must_inline("answer");
        let module = vm.global_module();
        let slot = vm
            .heap_mut()
            .modules
            .get_mut(module)
            .unwrap()
            .define(symbol);
        let ast = must_parse("answer", &bump, vm.heap_mut());

        assert_eq!(
            compile(&ast, &mut vm).unwrap().bytecode.as_ref(),
            &[Instruction::LookupSlot(slot), Instruction::Return],
        );
    }

    #[test]
    fn lookup_inlines_global_values_as_constants() {
        let bump = Bump::new();
        let mut vm = Vm::new();
        let module = vm
            .create_root_module(ModuleSource::Internal("test"))
            .unwrap();
        vm.set_global(Symbol::must_inline("answer"), Val::Int(42));
        let ast = must_parse("answer", &bump, vm.heap_mut());

        assert_eq!(
            compile_in_module(&ast, &mut vm, module)
                .unwrap()
                .bytecode
                .as_ref(),
            &[Instruction::PushConst(Val::Int(42)), Instruction::Return],
        );
    }

    #[test]
    fn lookup_of_unbound_symbol_is_a_compilation_error() {
        let bump = Bump::new();
        let mut vm = Vm::default();
        let module = vm
            .create_root_module(ModuleSource::Internal("test"))
            .unwrap();
        let ast = must_parse("answer", &bump, vm.heap_mut());

        let error =
            compile_in_module(&ast, &mut vm, module).expect_err("expected an unbound symbol error");
        assert_eq!(
            error,
            crate::error::CompilerError {
                kind: crate::error::CompilerErrorKind::UnboundSymbol(Symbol::must_inline("answer")),
                span: 0..6,
            }
        );
    }

    #[test]
    fn qualified_lookups_are_unsupported_in_all_scopes() {
        for source in [
            "ns/x",
            "(ns/x 1)",
            "(lambda (x) ns/x)",
            "(lambda (x) (lambda () ns/x))",
            "(let ((x 1)) ns/x)",
            "(define x 1) ns/x",
            "(define x ns/x)",
            "(set! x ns/x)",
        ] {
            let bump = Bump::new();
            let mut vm = Vm::new();
            vm.set_global(Symbol::must_inline("x"), Val::Int(42));
            let module = vm
                .create_root_module(ModuleSource::Internal("test"))
                .unwrap();
            vm.heap_mut()
                .modules
                .get_mut(module)
                .unwrap()
                .set(Symbol::must_inline("x"), Val::Int(1));
            let ast = must_parse(source, &bump, vm.heap_mut());
            let start = source.find("ns/x").unwrap();
            assert_eq!(
                compile_in_module(&ast, &mut vm, module).unwrap_err(),
                CompilerError {
                    kind: crate::error::CompilerErrorKind::UnsupportedNamespace(
                        Symbol::must_inline("ns")
                    ),
                    span: start..start + 4,
                },
                "{source}"
            );
        }
    }

    #[test]
    fn import_in_any_position_is_error() {
        for (source, span) in [
            ("(import \"path/to/file.tk\" as file)", 0..34),
            ("(define x (import \"a.tk\"))", 10..25),
            ("(lambda () (import \"a.tk\"))", 11..26),
        ] {
            let bump = Bump::new();
            let mut vm = Vm::default();
            let module = vm
                .create_root_module(ModuleSource::Internal("test"))
                .unwrap();
            let ast = must_parse(source, &bump, vm.heap_mut());

            assert_eq!(
                compile_in_module(&ast, &mut vm, module).unwrap_err(),
                CompilerError {
                    kind: crate::error::CompilerErrorKind::UnsupportedImport,
                    span,
                },
                "{source}",
            );
        }
    }

    #[test]
    fn let_compiles_bindings_to_reserved_local_slots() {
        let bump = Bump::new();
        let mut vm = Vm::new();
        let module = vm
            .create_root_module(ModuleSource::Internal("test"))
            .unwrap();
        let plus = vm.get_global(Symbol::must_inline("+"));
        let ast = must_parse("(let ((x 1) (y (+ x 2))) (+ x y))", &bump, vm.heap_mut());
        let function = compile_in_module(&ast, &mut vm, module).unwrap();

        assert_eq!(function.local_count, 2);
        assert_eq!(
            function.bytecode.as_ref(),
            &[
                Instruction::PushConst(Val::Int(1)),
                Instruction::SetLocal(0),
                Instruction::PushConst(plus),
                Instruction::GetLocal(0),
                Instruction::PushConst(Val::Int(2)),
                Instruction::Eval(2),
                Instruction::SetLocal(1),
                Instruction::PushConst(plus),
                Instruction::GetLocal(0),
                Instruction::GetLocal(1),
                Instruction::Eval(2),
                Instruction::Return,
            ],
        );
    }

    #[test]
    fn lambda_compiles_to_bytecode_function_constant() {
        let bump = Bump::new();
        let mut vm = Vm::default();
        let ast = must_parse("(lambda (x y) 42)", &bump, vm.heap_mut());
        let function = compile(&ast, &mut vm).unwrap();

        let [
            Instruction::PushConst(Val::BytecodeFunction(handle)),
            Instruction::Return,
        ] = function.bytecode.as_ref()
        else {
            panic!("expected lambda constant and return")
        };
        let handle = *handle;
        assert_eq!(
            vm.heap().bytecode_functions.get(handle).unwrap(),
            &BytecodeFunction {
                module: vm.global_module(),
                name: None,
                arity: Arity::Exact(2),
                local_count: 0,
                captures: [].into(),
                bytecode: [Instruction::PushConst(Val::Int(42)), Instruction::Return].into(),
            }
        );
    }

    #[test]
    fn defined_lambda_has_definition_name() {
        let bump = Bump::new();
        let mut vm = Vm::default();
        let ast = must_parse("(define answer (lambda () 42))", &bump, vm.heap_mut());
        let function = compile(&ast, &mut vm).unwrap();

        let [
            Instruction::PushConst(Val::BytecodeFunction(handle)),
            Instruction::Define(_),
            Instruction::Return,
        ] = function.bytecode.as_ref()
        else {
            panic!("expected named lambda definition")
        };
        let handle = *handle;

        assert_eq!(
            vm.heap().bytecode_functions.get(handle).unwrap(),
            &BytecodeFunction {
                module: vm.global_module(),
                name: Some(Symbol::must_inline("answer")),
                arity: Arity::Exact(0),
                local_count: 0,
                captures: [].into(),
                bytecode: [Instruction::PushConst(Val::Int(42)), Instruction::Return].into(),
            }
        );
    }

    #[test]
    fn lambda_arguments_compile_to_local_get_instructions() {
        let bump = Bump::new();
        let mut vm = Vm::new();
        let module = vm
            .create_root_module(ModuleSource::Internal("test"))
            .unwrap();
        let plus = vm.get_global(Symbol::must_inline("+"));
        let ast = must_parse("(lambda (x y) (+ x y))", &bump, vm.heap_mut());
        let function = compile_in_module(&ast, &mut vm, module).unwrap();

        let [
            Instruction::PushConst(Val::BytecodeFunction(handle)),
            Instruction::Return,
        ] = function.bytecode.as_ref()
        else {
            panic!("expected lambda constant and return")
        };
        let handle = *handle;

        assert_eq!(
            vm.heap().bytecode_functions.get(handle).unwrap(),
            &BytecodeFunction {
                module,
                name: None,
                arity: Arity::Exact(2),
                local_count: 0,
                captures: [].into(),
                bytecode: [
                    Instruction::PushConst(plus),
                    Instruction::GetLocal(0),
                    Instruction::GetLocal(1),
                    Instruction::Eval(2),
                    Instruction::Return,
                ]
                .into(),
            }
        );
    }

    #[test]
    fn nested_lambda_only_sees_its_own_arguments() {
        let bump = Bump::new();
        let mut vm = Vm::default();
        let ast = must_parse(
            "(lambda (outer) (lambda (inner) inner))",
            &bump,
            vm.heap_mut(),
        );
        let function = compile(&ast, &mut vm).unwrap();

        let [
            Instruction::PushConst(Val::BytecodeFunction(outer_handle)),
            Instruction::Return,
        ] = function.bytecode.as_ref()
        else {
            panic!("expected lambda constant and return")
        };
        let outer_handle = *outer_handle;
        let outer = vm.heap().bytecode_functions.get(outer_handle).unwrap();
        let [
            Instruction::PushConst(Val::BytecodeFunction(inner_handle)),
            Instruction::Return,
        ] = outer.bytecode.as_ref()
        else {
            panic!("expected nested lambda constant and return")
        };

        assert_eq!(
            outer,
            &BytecodeFunction {
                module: vm.global_module(),
                name: None,
                arity: Arity::Exact(1),
                local_count: 0,
                captures: [].into(),
                bytecode: [
                    Instruction::PushConst(Val::BytecodeFunction(*inner_handle)),
                    Instruction::Return,
                ]
                .into(),
            }
        );
        assert_eq!(
            vm.heap().bytecode_functions.get(*inner_handle).unwrap(),
            &BytecodeFunction {
                module: vm.global_module(),
                name: None,
                arity: Arity::Exact(1),
                local_count: 0,
                captures: [].into(),
                bytecode: [Instruction::GetLocal(0), Instruction::Return].into(),
            }
        );
    }

    #[test]
    fn anonymous_lambda_inside_function_call_has_no_name() {
        let bump = Bump::new();
        let mut vm = Vm::default();
        let ast = must_parse("((lambda (x) x) 42)", &bump, vm.heap_mut());
        let function = compile(&ast, &mut vm).unwrap();

        let [
            Instruction::PushConst(Val::BytecodeFunction(handle)),
            Instruction::PushConst(Val::Int(42)),
            Instruction::Eval(1),
            Instruction::Return,
        ] = function.bytecode.as_ref()
        else {
            panic!("expected anonymous lambda call")
        };
        let handle = *handle;

        assert_eq!(
            vm.heap().bytecode_functions.get(handle).unwrap(),
            &BytecodeFunction {
                module: vm.global_module(),
                name: None,
                arity: Arity::Exact(1),
                local_count: 0,
                captures: [].into(),
                bytecode: [Instruction::GetLocal(0), Instruction::Return].into(),
            }
        );
    }

    #[test]
    fn nested_lambda_captures_an_enclosing_argument() {
        let bump = Bump::new();
        let mut vm = Vm::default();
        let ast = must_parse("(lambda (x) (lambda () x))", &bump, vm.heap_mut());
        let function = compile(&ast, &mut vm).unwrap();

        let [
            Instruction::PushConst(Val::BytecodeFunction(outer_handle)),
            Instruction::Return,
        ] = function.bytecode.as_ref()
        else {
            panic!("expected outer lambda")
        };
        let outer_handle = *outer_handle;
        let outer = vm.heap().bytecode_functions.get(outer_handle).unwrap();
        let [Instruction::MakeClosure(inner_handle), Instruction::Return] = outer.bytecode.as_ref()
        else {
            panic!("expected inner lambda")
        };

        assert_eq!(outer.captures.as_ref(), &[]);
        assert_eq!(
            vm.heap()
                .bytecode_functions
                .get(*inner_handle)
                .unwrap()
                .captures
                .as_ref(),
            &[CaptureSource::Local(0)]
        );
    }

    #[test]
    fn transitive_lambda_capture_uses_parent_upvalue() {
        let bump = Bump::new();
        let mut vm = Vm::default();
        let ast = must_parse(
            "(lambda (x) (lambda () (lambda () x)))",
            &bump,
            vm.heap_mut(),
        );
        let function = compile(&ast, &mut vm).unwrap();

        let [
            Instruction::PushConst(Val::BytecodeFunction(outer_handle)),
            Instruction::Return,
        ] = function.bytecode.as_ref()
        else {
            panic!("expected outer lambda")
        };
        let outer_handle = *outer_handle;
        let outer = vm.heap().bytecode_functions.get(outer_handle).unwrap();
        let [Instruction::MakeClosure(middle_handle), Instruction::Return] =
            outer.bytecode.as_ref()
        else {
            panic!("expected middle lambda")
        };
        let middle = vm.heap().bytecode_functions.get(*middle_handle).unwrap();
        let [Instruction::MakeClosure(inner_handle), Instruction::Return] =
            middle.bytecode.as_ref()
        else {
            panic!("expected inner lambda")
        };

        assert_eq!(middle.captures.as_ref(), &[CaptureSource::Local(0)]);
        assert_eq!(
            vm.heap()
                .bytecode_functions
                .get(*inner_handle)
                .unwrap()
                .captures
                .as_ref(),
            &[CaptureSource::Upvalue(0)]
        );
    }

    #[test]
    fn nested_lambda_capture_respects_shadowed_local() {
        let bump = Bump::new();
        let mut vm = Vm::default();
        let ast = must_parse(
            "(lambda (x) (let ((x 2)) (lambda () x)))",
            &bump,
            vm.heap_mut(),
        );
        let function = compile(&ast, &mut vm).unwrap();

        let [
            Instruction::PushConst(Val::BytecodeFunction(outer_handle)),
            Instruction::Return,
        ] = function.bytecode.as_ref()
        else {
            panic!("expected outer lambda")
        };
        let outer_handle = *outer_handle;
        let outer = vm.heap().bytecode_functions.get(outer_handle).unwrap();
        let [
            Instruction::PushConst(Val::Int(2)),
            Instruction::SetLocal(1),
            Instruction::MakeClosure(inner_handle),
            Instruction::Return,
        ] = outer.bytecode.as_ref()
        else {
            panic!("expected shadowing local and inner lambda")
        };

        assert_eq!(outer.local_count, 1);
        assert_eq!(
            vm.heap()
                .bytecode_functions
                .get(*inner_handle)
                .unwrap()
                .captures
                .as_ref(),
            &[CaptureSource::Local(1)]
        );
    }

    #[test]
    fn nested_lambda_capture_of_named_function_does_not_reach_top_level() {
        let bump = Bump::new();
        let mut vm = Vm::default();
        let ast = must_parse("(define f (lambda () (lambda () f)))", &bump, vm.heap_mut());
        let function = compile(&ast, &mut vm).unwrap();

        let [
            Instruction::PushConst(Val::BytecodeFunction(outer_handle)),
            ..,
        ] = function.bytecode.as_ref()
        else {
            panic!("expected named outer lambda")
        };
        let outer_handle = *outer_handle;
        let outer = vm.heap().bytecode_functions.get(outer_handle).unwrap();
        let [Instruction::MakeClosure(inner_handle), Instruction::Return] = outer.bytecode.as_ref()
        else {
            panic!("expected inner lambda")
        };

        assert_eq!(outer.captures.as_ref(), &[]);
        assert_eq!(
            vm.heap()
                .bytecode_functions
                .get(*inner_handle)
                .unwrap()
                .captures
                .as_ref(),
            &[CaptureSource::CurrentFunction]
        );
    }

    #[test]
    fn unexpected_closing_paren_reports_read_error_with_span() {
        assert_eq!(
            must_error(" )").as_slice(),
            &[AstError {
                kind: AstErrorKind::Read(ReadErrorKind::UnexpectedEndParen),
                span: 1..2,
            }],
        );
    }

    #[test]
    fn unclosed_list_reports_read_error_with_span() {
        assert_eq!(
            must_error(" (foo 1").as_slice(),
            &[AstError {
                kind: AstErrorKind::Read(ReadErrorKind::MissingEndParen),
                span: 1..2,
            }],
        );
    }

    #[test]
    fn malformed_string_reports_read_error_with_span() {
        assert_eq!(
            must_error(" \"abc").as_slice(),
            &[AstError {
                kind: AstErrorKind::Read(ReadErrorKind::MalformedString),
                span: 1..5,
            }],
        );
    }

    #[test]
    fn form_error_followed_by_read_error_preserves_both_errors() {
        assert_eq!(
            must_error("(foo ()").as_slice(),
            &[
                AstError {
                    kind: AstErrorKind::Form(FormErrorKind::EmptyExpression),
                    span: 5..7,
                },
                AstError {
                    kind: AstErrorKind::Read(ReadErrorKind::MissingEndParen),
                    span: 0..1,
                },
            ],
        );
    }
}
