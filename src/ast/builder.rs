use bumpalo::{Bump, collections::Vec as BumpVec};

use crate::heap::Heap;
use crate::reader::Atom;
use crate::source_map::SourceId;
use crate::{
    ast::{Ast, AstKind, LetBinding},
    reader::{ReadErrorKind, ReadVisitor},
    symbol::Symbol,
    tokenizer::Span,
    val::Val,
};

pub struct AstBuilder<'a, 'b> {
    bump: &'a Bump,
    heap: &'b mut Heap,
    results: BumpVec<'a, Ast<'a>>,
    expr: BumpVec<'a, ExprFrame<'a>>,
    errors: AstErrors,
}

/// State for a list that is currently being read.
///
/// `accepts_empty` belongs to the child list being read, so the decision is
/// made when that list starts, while its parent context is still available.
struct ExprFrame<'a> {
    start: usize,
    accepts_empty: bool,
    expr: BumpVec<'a, Ast<'a>>,
}

/// Can collect several `AstError` objects.
#[derive(Clone, Debug, PartialEq)]
pub struct AstErrors {
    pub(crate) source: SourceId,
    errors: Vec<AstError>,
}

/// An error that occurs when building an Ast.
#[derive(Clone, Debug, PartialEq)]
pub struct AstError {
    /// The kind of error that occurred.
    pub kind: AstErrorKind,
    /// The location of the error within the source.
    pub span: Span,
}

/// An error that occurs due to a malformed `Ast`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum FormErrorKind {
    /// No expression was found in the source.
    NoExpression,
    /// Corresponds to `()` which cannot be evaluated.
    EmptyExpression,
    /// A qualified symbol had no name after its final namespace separator.
    EmptySymbolName,
    /// A `lambda` form was missing its parameter list and body.
    LambdaMissingParameterListAndBody,
    /// A `lambda` form was missing its body.
    LambdaMissingBody,
    /// A `let` form was missing its binding list and body.
    LetMissingBindingListAndBody,
    /// A `let` form was missing its body.
    LetMissingBody,
    /// An `if` form was missing its condition and branches.
    IfMissingConditionAndBranches,
    /// An `if` form was missing its branches.
    IfMissingBranches,
    /// An `if` form had too many operands.
    IfTooManyArguments(usize),
    /// A `when` form was missing its condition and body.
    WhenMissingConditionAndBody,
    /// A `return` form had too many operands.
    ReturnTooManyArguments(usize),
    /// A `define` form was missing its name and value or function definition.
    DefineMissingNameAndValueOrFunctionDefinition,
    /// A `define` form was missing its value or function body.
    DefineMissingValueOrFunctionBody,
    /// A `define` form had too many operands.
    DefineTooManyArguments(usize),
    /// A `set!` form was missing its name and value.
    SetMissingNameAndValue,
    /// A `set!` form was missing its value.
    SetMissingValue,
    /// A `set!` form had too many operands.
    SetTooManyArguments(usize),
    /// A `set!` form contained a name that was not an unqualified symbol.
    SetInvalidName,
    /// A `lambda` form contained a parameter that was not an unqualified symbol.
    LambdaInvalidParameter,
    /// A `let` form contained a binding name that was not an unqualified symbol.
    LetInvalidBindingName,
    /// A `define` form contained a name that was not an unqualified symbol.
    DefineInvalidName,
    /// A `let` binding had the wrong number of elements.
    InvalidBindingArity(usize),
    /// A `let` form did not contain a binding list.
    LetInvalidBindingList,
    /// A `let` binding was not a name/value pair.
    LetInvalidBinding,
}

/// The kind of error that occurred.
#[derive(Clone, Debug, PartialEq)]
pub enum AstErrorKind {
    /// An error that occurred when reading the source string.
    Read(ReadErrorKind),
    /// An error that occurred when forming the `Ast`.
    Form(FormErrorKind),
}

impl AstErrors {
    pub(crate) fn new(source: SourceId) -> AstErrors {
        AstErrors {
            source,
            errors: Vec::new(),
        }
    }

    /// Add a new error into `self`.
    pub(crate) fn push(&mut self, kind: AstErrorKind, span: Span) {
        self.errors.push(AstError { kind, span });
    }

    /// Get all errors that have been collected.
    pub(crate) fn as_slice(&self) -> &[AstError] {
        &self.errors
    }
}

impl<'a, 'b> AstBuilder<'a, 'b> {
    pub fn new(source: SourceId, bump: &'a Bump, heap: &'b mut Heap) -> Self {
        AstBuilder {
            bump,
            heap,
            results: BumpVec::new_in(bump),
            expr: BumpVec::new_in(bump),
            errors: AstErrors::new(source),
        }
    }
}

impl ReadVisitor for AstBuilder<'_, '_> {
    fn visit_atom(&mut self, span: Span, atom: Atom) {
        let kind = match atom {
            Atom::Symbol(sym) => {
                // A bare slash is the division operator, not a namespace separator.
                let (namespace, symbol) = if sym == "/" {
                    (None, sym)
                } else {
                    match sym.rsplit_once('/') {
                        Some((namespace, symbol)) => (Some(namespace), symbol),
                        None => (None, sym),
                    }
                };
                if symbol.is_empty() {
                    self.errors.push(
                        AstErrorKind::Form(FormErrorKind::EmptySymbolName),
                        span.clone(),
                    );
                    AstKind::default()
                } else {
                    AstKind::Lookup {
                        namespace: namespace.map(|name| self.heap.symbols.create_symbol(name)),
                        symbol: self.heap.symbols.create_symbol(symbol),
                    }
                }
            }
            atom => AstKind::Constant(atom.to_val(self.heap)),
        };
        match self.expr.last_mut() {
            Some(frame) => frame.expr.push(Ast { kind, span }),
            None => self.results.push(Ast { kind, span }),
        }
    }

    fn start_list(&mut self, span: Span) {
        let accepts_empty = self.expr.last().is_some_and(|frame| {
            matches!(
                frame.expr.as_slice(),
                [Ast {
                    kind: AstKind::Lookup {
                        namespace: None,
                        symbol: Symbol::LAMBDA | Symbol::LET
                    },
                    ..
                }]
            )
        });
        self.expr.push(ExprFrame {
            start: span.start,
            accepts_empty,
            expr: BumpVec::new_in(self.bump),
        });
    }

    fn end_list(&mut self, span: Span) {
        let frame = match self.expr.pop() {
            Some(frame) => frame,
            None => {
                // end_list calls always matches start_List.
                std::hint::cold_path();
                return;
            }
        };
        let span = frame.start..span.end;
        let expr = frame.expr.into_bump_slice();
        let kind = match expr {
            [lead_expr, arg_exprs @ ..] => self.expr_to_ast(lead_expr, arg_exprs, span.clone()),
            [] if frame.accepts_empty => AstKind::Begin(&[]),
            [] => {
                std::hint::cold_path();
                self.errors.push(
                    AstErrorKind::Form(FormErrorKind::EmptyExpression),
                    span.clone(),
                );
                AstKind::default()
            }
        };
        match self.expr.last_mut() {
            Some(frame) => frame.expr.push(Ast { kind, span }),
            None => self.results.push(Ast { kind, span }),
        }
    }

    fn visit_comment(&mut self, _span: Span, _comment: &str) {}
}

impl<'a, 'b> AstBuilder<'a, 'b> {
    pub fn push_error(&mut self, error_kind: AstErrorKind, span: Span) {
        self.errors.push(error_kind, span);
    }

    pub fn into_ast(mut self, source_span: Span) -> Result<Ast<'a>, AstErrors> {
        if self.results.is_empty() && self.errors.as_slice().is_empty() {
            std::hint::cold_path();
            self.errors.push(
                AstErrorKind::Form(FormErrorKind::NoExpression),
                source_span.clone(),
            );
        }

        if !self.errors.as_slice().is_empty() {
            std::hint::cold_path();
            return Err(self.errors);
        }

        if self.results.len() == 1 {
            return Ok(self.results.pop().unwrap());
        }

        Ok(Ast {
            kind: AstKind::Begin(self.results.into_bump_slice()),
            span: source_span,
        })
    }

    fn expr_to_ast(
        &mut self,
        lead_expr: &'a Ast<'a>,
        arg_exprs: &'a [Ast<'a>],
        span: Span,
    ) -> AstKind<'a> {
        if let AstKind::Lookup {
            namespace: None,
            symbol,
        } = &lead_expr.kind
        {
            match *symbol {
                Symbol::BEGIN => return AstKind::Begin(arg_exprs),
                Symbol::IF => return self.if_to_ast(arg_exprs, span),
                Symbol::WHEN => return self.when_to_ast(arg_exprs, span),
                Symbol::RETURN => return self.return_to_ast(arg_exprs, span),
                Symbol::DEFINE => return self.define_to_ast(arg_exprs, span),
                Symbol::SET_B => return self.set_to_ast(arg_exprs, span),
                Symbol::LAMBDA => return self.lambda_to_ast(arg_exprs, span),
                Symbol::LET => return self.let_to_ast(arg_exprs, span),
                _ => {}
            }
        }

        AstKind::FunctionCall {
            function: lead_expr,
            args: arg_exprs,
        }
    }

    fn let_to_ast(&mut self, arg_exprs: &'a [Ast<'a>], span: Span) -> AstKind<'a> {
        let [bindings_expr, body_exprs @ ..] = arg_exprs else {
            self.errors.push(
                AstErrorKind::Form(FormErrorKind::LetMissingBindingListAndBody),
                span,
            );
            return AstKind::default();
        };
        if body_exprs.is_empty() {
            self.errors
                .push(AstErrorKind::Form(FormErrorKind::LetMissingBody), span);
            return AstKind::default();
        }
        let binding_exprs: Vec<&Ast<'a>> = match &bindings_expr.kind {
            AstKind::Begin(binding_exprs) => binding_exprs.iter().collect(),
            AstKind::FunctionCall { function, args } => {
                std::iter::once(*function).chain(args.iter()).collect()
            }
            _ => {
                self.errors.push(
                    AstErrorKind::Form(FormErrorKind::LetInvalidBindingList),
                    bindings_expr.span.clone(),
                );
                return AstKind::default();
            }
        };
        let mut bindings = BumpVec::with_capacity_in(binding_exprs.len(), self.bump);
        for binding in binding_exprs {
            let AstKind::FunctionCall { function, args } = &binding.kind else {
                self.errors.push(
                    AstErrorKind::Form(FormErrorKind::LetInvalidBinding),
                    binding.span.clone(),
                );
                continue;
            };
            let AstKind::Lookup {
                namespace: None,
                symbol,
            } = function.kind
            else {
                self.errors.push(
                    AstErrorKind::Form(FormErrorKind::LetInvalidBindingName),
                    function.span.clone(),
                );
                continue;
            };
            let [value] = *args else {
                self.errors.push(
                    AstErrorKind::Form(FormErrorKind::InvalidBindingArity(args.len() + 1)),
                    binding.span.clone(),
                );
                continue;
            };
            bindings.push(LetBinding { symbol, value });
        }
        let body = if let [body] = body_exprs {
            body
        } else {
            self.bump.alloc(Ast {
                kind: AstKind::Begin(body_exprs),
                span: body_exprs.first().unwrap().span.start..body_exprs.last().unwrap().span.end,
            })
        };
        AstKind::Let {
            bindings: bindings.into_bump_slice(),
            body: self.bump.alloc(body),
        }
    }

    fn if_to_ast(&mut self, arg_exprs: &'a [Ast<'a>], span: Span) -> AstKind<'a> {
        match arg_exprs {
            [condition, then_branch] => AstKind::If {
                condition,
                then_branch,
                else_branch: self.bump.alloc(Ast {
                    kind: AstKind::Constant(Val::Undefined),
                    span,
                }),
            },
            [condition, then_branch, else_branch] => AstKind::If {
                condition,
                then_branch,
                else_branch,
            },
            rest => {
                std::hint::cold_path();
                self.errors.push(
                    AstErrorKind::Form(match rest.len() {
                        0 => FormErrorKind::IfMissingConditionAndBranches,
                        1 => FormErrorKind::IfMissingBranches,
                        found => FormErrorKind::IfTooManyArguments(found),
                    }),
                    span,
                );
                AstKind::default()
            }
        }
    }

    fn when_to_ast(&mut self, arg_exprs: &'a [Ast<'a>], span: Span) -> AstKind<'a> {
        match arg_exprs {
            [condition, then_exprs @ ..] => AstKind::If {
                condition,
                then_branch: self.bump.alloc(Ast {
                    kind: AstKind::Begin(then_exprs),
                    span: span.clone(),
                }),
                else_branch: self.bump.alloc(Ast {
                    kind: AstKind::Constant(Val::Undefined),
                    span,
                }),
            },
            _ => {
                std::hint::cold_path();
                self.errors.push(
                    AstErrorKind::Form(FormErrorKind::WhenMissingConditionAndBody),
                    span,
                );
                AstKind::default()
            }
        }
    }

    fn return_to_ast(&mut self, arg_exprs: &'a [Ast<'a>], span: Span) -> AstKind<'a> {
        match arg_exprs {
            [] => AstKind::Return(self.bump.alloc(Ast {
                kind: AstKind::Constant(Val::Undefined),
                span,
            })),
            [value] => AstKind::Return(value),
            rest => {
                std::hint::cold_path();
                self.errors.push(
                    AstErrorKind::Form(FormErrorKind::ReturnTooManyArguments(rest.len())),
                    span,
                );
                AstKind::default()
            }
        }
    }

    fn define_to_ast(&mut self, arg_exprs: &'a [Ast<'a>], span: Span) -> AstKind<'a> {
        match arg_exprs {
            [binding, value] => self.define_value_to_ast(binding, value, span),
            [
                Ast {
                    span: binding_span,
                    kind: AstKind::FunctionCall { function, args },
                },
                body_exprs @ ..,
            ] => {
                let binding_span = binding_span.clone();
                let args = *args;
                let AstKind::Lookup {
                    namespace: None,
                    symbol: binding_symbol,
                } = function.kind
                else {
                    std::hint::cold_path();
                    self.errors.push(
                        AstErrorKind::Form(FormErrorKind::DefineInvalidName),
                        function.span.clone(),
                    );
                    return AstKind::default();
                };
                let lambda_span = binding_span.start..span.end;
                let lambda = self.lambda_to_ast_owned(args, body_exprs, lambda_span.clone());
                AstKind::Define {
                    symbol: binding_symbol,
                    value: self.bump.alloc(Ast {
                        kind: lambda,
                        span: lambda_span,
                    }),
                }
            }
            rest => {
                std::hint::cold_path();
                self.errors.push(
                    AstErrorKind::Form(match rest.len() {
                        0 => FormErrorKind::DefineMissingNameAndValueOrFunctionDefinition,
                        1 => FormErrorKind::DefineMissingValueOrFunctionBody,
                        found => FormErrorKind::DefineTooManyArguments(found),
                    }),
                    span,
                );
                AstKind::default()
            }
        }
    }

    fn set_to_ast(&mut self, arg_exprs: &'a [Ast<'a>], span: Span) -> AstKind<'a> {
        match arg_exprs {
            [binding, value] => {
                let AstKind::Lookup {
                    namespace: None,
                    symbol,
                } = binding.kind
                else {
                    std::hint::cold_path();
                    self.errors.push(
                        AstErrorKind::Form(FormErrorKind::SetInvalidName),
                        binding.span.clone(),
                    );
                    return AstKind::default();
                };
                AstKind::Set { symbol, value }
            }
            rest => {
                std::hint::cold_path();
                let form = match rest.len() {
                    0 => FormErrorKind::SetMissingNameAndValue,
                    1 => FormErrorKind::SetMissingValue,
                    found => FormErrorKind::SetTooManyArguments(found),
                };
                self.errors.push(AstErrorKind::Form(form), span);
                AstKind::default()
            }
        }
    }

    fn define_value_to_ast(
        &mut self,
        binding: &'a Ast<'a>,
        value: &'a Ast<'a>,
        span: Span,
    ) -> AstKind<'a> {
        let binding_span = binding.span.clone();
        match binding.kind {
            AstKind::Lookup {
                namespace: None,
                symbol: binding_symbol,
            } => AstKind::Define {
                symbol: binding_symbol,
                value,
            },
            AstKind::FunctionCall { function, args } => {
                let AstKind::Lookup {
                    namespace: None,
                    symbol: binding_symbol,
                } = function.kind
                else {
                    std::hint::cold_path();
                    self.errors.push(
                        AstErrorKind::Form(FormErrorKind::DefineInvalidName),
                        function.span.clone(),
                    );
                    return AstKind::default();
                };
                let lambda_span = binding_span.start..span.end;
                let lambda = self.lambda_to_ast_owned(
                    args,
                    std::slice::from_ref(value),
                    lambda_span.clone(),
                );
                AstKind::Define {
                    symbol: binding_symbol,
                    value: self.bump.alloc(Ast {
                        kind: lambda,
                        span: lambda_span,
                    }),
                }
            }
            _ => {
                std::hint::cold_path();
                self.errors.push(
                    AstErrorKind::Form(FormErrorKind::DefineInvalidName),
                    binding_span,
                );
                AstKind::default()
            }
        }
    }

    fn lambda_to_ast_owned(
        &mut self,
        params: &[Ast<'a>],
        body_exprs: &'a [Ast<'a>],
        span: Span,
    ) -> AstKind<'a> {
        let mut args = BumpVec::with_capacity_in(params.len(), self.bump);
        let mut invalid_param = false;
        for param in params {
            let AstKind::Lookup {
                namespace: None,
                symbol,
            } = param.kind
            else {
                std::hint::cold_path();
                self.errors.push(
                    AstErrorKind::Form(FormErrorKind::LambdaInvalidParameter),
                    param.span.clone(),
                );
                invalid_param = true;
                continue;
            };
            args.push(symbol);
        }
        if invalid_param {
            return AstKind::default();
        }
        if body_exprs.is_empty() {
            std::hint::cold_path();
            self.errors
                .push(AstErrorKind::Form(FormErrorKind::LambdaMissingBody), span);
            return AstKind::default();
        }
        self.lambda_body(args.into_bump_slice(), body_exprs)
    }

    fn lambda_to_ast(&mut self, arg_exprs: &'a [Ast<'a>], span: Span) -> AstKind<'a> {
        let [params, body_exprs @ ..] = arg_exprs else {
            std::hint::cold_path();
            self.errors.push(
                AstErrorKind::Form(FormErrorKind::LambdaMissingParameterListAndBody),
                span,
            );
            return AstKind::default();
        };
        if body_exprs.is_empty() {
            std::hint::cold_path();
            self.errors
                .push(AstErrorKind::Form(FormErrorKind::LambdaMissingBody), span);
            return AstKind::default();
        }

        let args = match &params.kind {
            AstKind::Begin([]) => BumpVec::new_in(self.bump),
            AstKind::FunctionCall { function, args } => {
                let mut symbols = BumpVec::with_capacity_in(args.len() + 1, self.bump);
                let params = std::iter::once(*function).chain(args.iter());
                let mut invalid_param = false;
                for param in params {
                    let AstKind::Lookup {
                        namespace: None,
                        symbol,
                    } = param.kind
                    else {
                        std::hint::cold_path();
                        self.errors.push(
                            AstErrorKind::Form(FormErrorKind::LambdaInvalidParameter),
                            param.span.clone(),
                        );
                        invalid_param = true;
                        continue;
                    };
                    symbols.push(symbol);
                }
                if invalid_param {
                    return AstKind::default();
                }
                symbols
            }
            _ => {
                std::hint::cold_path();
                self.errors.push(
                    AstErrorKind::Form(FormErrorKind::LambdaInvalidParameter),
                    params.span.clone(),
                );
                return AstKind::default();
            }
        };

        self.lambda_body(args.into_bump_slice(), body_exprs)
    }

    fn lambda_body(&mut self, args: &'a [Symbol], body_exprs: &'a [Ast<'a>]) -> AstKind<'a> {
        let body = if let [body] = body_exprs {
            body
        } else {
            let span = body_exprs.first().unwrap().span.start..body_exprs.last().unwrap().span.end;
            self.bump.alloc(Ast {
                kind: AstKind::Begin(body_exprs),
                span,
            })
        };
        AstKind::Lambda { args, body }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{heap::Heap, symbol::Symbol, val::Val};
    use bumpalo::Bump;
    use std::assert_matches;

    fn must_parse<'a>(source: &str, bump: &'a Bump, heap: &mut Heap) -> Ast<'a> {
        Ast::parse(SourceId::new_unique(), source, bump, heap).expect("expected valid AST")
    }

    fn must_error(source: &str) -> AstErrors {
        let bump = Bump::new();
        Ast::parse(SourceId::new_unique(), source, &bump, &mut Heap::default())
            .expect_err("expected errors")
    }

    #[test]
    fn literal_preserves_value_and_span() {
        let bump = Bump::new();
        let mut heap = Heap::default();

        assert_eq!(
            must_parse(" 42 ", &bump, &mut heap),
            Ast {
                kind: AstKind::Constant(Val::Int(42)),
                span: 1..3
            }
        );
        assert_eq!(
            must_parse(" -3.5 ", &bump, &mut heap),
            Ast {
                kind: AstKind::Constant(Val::Float(-3.5)),
                span: 1..5
            }
        );
        assert_eq!(
            must_parse(" #true ", &bump, &mut heap),
            Ast {
                kind: AstKind::Constant(Val::Bool(true)),
                span: 1..6
            }
        );
        assert_eq!(
            must_parse(" #false ", &bump, &mut heap),
            Ast {
                kind: AstKind::Constant(Val::Bool(false)),
                span: 1..7
            }
        );
        assert_eq!(
            must_parse(" nil ", &bump, &mut Heap::default()),
            Ast {
                kind: AstKind::Constant(Val::Nil),
                span: 1..4,
            }
        );

        assert_matches!(
            must_parse(" \"a\\nb\" ", &bump, &mut heap),
            Ast {
                kind: AstKind::Constant(Val::String(id)),
                span,
            } if span == (1..7) && heap.strings.get(id).unwrap() == "a\nb"
        );
    }

    #[test]
    fn symbol_produces_lookup_with_symbol_and_span() {
        let bump = Bump::new();
        let mut heap = Heap::default();

        assert_eq!(
            must_parse(" x ", &bump, &mut heap),
            Ast {
                kind: AstKind::Lookup {
                    namespace: None,
                    symbol: Symbol::must_inline("x")
                },
                span: 1..2
            }
        );

        let symbol = heap.symbols.create_symbol("long_symbol_name");
        assert_eq!(
            must_parse(" long_symbol_name ", &bump, &mut heap),
            Ast {
                kind: AstKind::Lookup {
                    namespace: None,
                    symbol
                },
                span: 1..17
            }
        );

        assert_eq!(
            must_parse(" λ ", &bump, &mut heap),
            Ast {
                kind: AstKind::Lookup {
                    namespace: None,
                    symbol: Symbol::must_inline("λ")
                },
                span: 1..3
            }
        );
    }

    #[test]
    fn qualified_symbols_preserve_namespace_symbol_and_span() {
        let bump = Bump::new();
        let mut heap = Heap::default();

        for (namespace, name) in [
            ("math", "add"),
            ("long_namespace_name", "long_symbol_name"),
            ("λ", "値"),
            ("x/y", "z"),
            ("long_namespace/child", "long_symbol_name"),
            ("λ/値", "名"),
        ] {
            let source = format!(" {namespace}/{name} ");
            let namespace = heap.symbols.create_symbol(namespace);
            let symbol = heap.symbols.create_symbol(name);
            assert_eq!(
                must_parse(&source, &bump, &mut heap),
                Ast {
                    kind: AstKind::Lookup {
                        namespace: Some(namespace),
                        symbol,
                    },
                    span: 1..source.len() - 1,
                }
            );
        }

        assert_eq!(
            must_parse(" / ", &bump, &mut heap),
            Ast {
                kind: AstKind::Lookup {
                    namespace: None,
                    symbol: Symbol::must_inline("/"),
                },
                span: 1..2,
            }
        );
    }

    #[test]
    fn qualified_symbols_require_a_nonempty_name() {
        for name in ["a/", "a/b/c/", "math//", "//", "λ/値/"] {
            let source = format!(" {name} ");
            assert_eq!(
                must_error(&source).as_slice(),
                &[AstError {
                    kind: AstErrorKind::Form(FormErrorKind::EmptySymbolName),
                    span: 1..source.len() - 1,
                }],
                "{source}"
            );
        }

        assert_eq!(
            must_error("(+ a/b/c/ d/)").as_slice(),
            &[
                AstError {
                    kind: AstErrorKind::Form(FormErrorKind::EmptySymbolName),
                    span: 3..9,
                },
                AstError {
                    kind: AstErrorKind::Form(FormErrorKind::EmptySymbolName),
                    span: 10..12,
                },
            ]
        );
    }

    #[test]
    fn qualified_special_form_names_are_function_calls() {
        let bump = Bump::new();
        let mut heap = Heap::default();
        for name in [
            "begin", "if", "when", "return", "define", "set!", "lambda", "let",
        ] {
            let source = format!("(ns/{name} x 42)");
            let ast = must_parse(&source, &bump, &mut heap);
            assert_matches!(
                ast.kind,
                AstKind::FunctionCall { function, args }
                if function.kind == AstKind::Lookup {
                    namespace: Some(Symbol::must_inline("ns")),
                    symbol: Symbol::must_inline(name),
                } && args.len() == 2
            );
        }
    }

    #[test]
    fn binding_names_and_parameters_forbid_namespaces() {
        for (source, expected) in [
            ("(define ns/x 1)", FormErrorKind::DefineInvalidName),
            ("(define (ns/x y) y)", FormErrorKind::DefineInvalidName),
            ("(define (ns/x y) 1 y)", FormErrorKind::DefineInvalidName),
            ("(set! ns/x 1)", FormErrorKind::SetInvalidName),
            ("(lambda (ns/x) 1)", FormErrorKind::LambdaInvalidParameter),
            ("(lambda (x ns/y) 1)", FormErrorKind::LambdaInvalidParameter),
            ("(define (f ns/x) 1)", FormErrorKind::LambdaInvalidParameter),
            (
                "(define (f x ns/y) 1 x)",
                FormErrorKind::LambdaInvalidParameter,
            ),
            ("(let ((ns/x 1)) 1)", FormErrorKind::LetInvalidBindingName),
            (
                "(let ((x 1) (ns/y 2)) x)",
                FormErrorKind::LetInvalidBindingName,
            ),
        ] {
            let start = source.find("ns/").unwrap();
            assert_eq!(
                must_error(source).as_slice(),
                &[AstError {
                    kind: AstErrorKind::Form(expected),
                    span: start..start + 4,
                }],
                "{source}"
            );
        }
    }

    #[test]
    fn single_element_list_produces_call_without_arguments() {
        let bump = Bump::new();
        assert_eq!(
            must_parse("(foo)", &bump, &mut Heap::default()),
            Ast {
                span: 0..5,
                kind: AstKind::FunctionCall {
                    function: &Ast {
                        span: 1..4,
                        kind: AstKind::Lookup {
                            namespace: None,
                            symbol: Symbol::must_inline("foo")
                        },
                    },
                    args: &[],
                },
            },
        );
    }

    #[test]
    fn multiple_element_list_preserves_function_and_argument_order() {
        let bump = Bump::new();
        assert_eq!(
            must_parse(" (foo 1 x 2) ", &bump, &mut Heap::default()),
            Ast {
                span: 1..12,
                kind: AstKind::FunctionCall {
                    function: &Ast {
                        span: 2..5,
                        kind: AstKind::Lookup {
                            namespace: None,
                            symbol: Symbol::must_inline("foo")
                        },
                    },
                    args: &[
                        Ast {
                            span: 6..7,
                            kind: AstKind::Constant(Val::Int(1))
                        },
                        Ast {
                            span: 8..9,
                            kind: AstKind::Lookup {
                                namespace: None,
                                symbol: Symbol::must_inline("x")
                            }
                        },
                        Ast {
                            span: 10..11,
                            kind: AstKind::Constant(Val::Int(2))
                        },
                    ],
                },
            },
        );
    }

    #[test]
    fn nested_calls_preserve_structure_and_spans() {
        let bump = Bump::new();
        assert_eq!(
            must_parse("(foo (bar 1) 2)", &bump, &mut Heap::default()),
            Ast {
                span: 0..15,
                kind: AstKind::FunctionCall {
                    function: &Ast {
                        span: 1..4,
                        kind: AstKind::Lookup {
                            namespace: None,
                            symbol: Symbol::must_inline("foo")
                        },
                    },
                    args: &[
                        Ast {
                            span: 5..12,
                            kind: AstKind::FunctionCall {
                                function: &Ast {
                                    span: 6..9,
                                    kind: AstKind::Lookup {
                                        namespace: None,
                                        symbol: Symbol::must_inline("bar")
                                    },
                                },
                                args: &[Ast {
                                    span: 10..11,
                                    kind: AstKind::Constant(Val::Int(1)),
                                }],
                            },
                        },
                        Ast {
                            span: 13..14,
                            kind: AstKind::Constant(Val::Int(2))
                        },
                    ],
                },
            },
        );
    }

    #[test]
    fn begin_list_produces_begin_expression() {
        let bump = Bump::new();
        assert_eq!(
            must_parse("(begin 1 2)", &bump, &mut Heap::default()),
            Ast {
                span: 0..11,
                kind: AstKind::Begin(&[
                    Ast {
                        span: 7..8,
                        kind: AstKind::Constant(Val::Int(1)),
                    },
                    Ast {
                        span: 9..10,
                        kind: AstKind::Constant(Val::Int(2)),
                    },
                ]),
            },
        );
    }

    #[test]
    fn return_list_produces_return_expression() {
        let bump = Bump::new();
        assert_eq!(
            must_parse("(return 42)", &bump, &mut Heap::default()),
            Ast {
                span: 0..11,
                kind: AstKind::Return(&Ast {
                    span: 8..10,
                    kind: AstKind::Constant(Val::Int(42)),
                }),
            },
        );
        assert_eq!(
            must_parse("(return)", &bump, &mut Heap::default()),
            Ast {
                span: 0..8,
                kind: AstKind::Return(&Ast {
                    span: 0..8,
                    kind: AstKind::Constant(Val::Undefined),
                }),
            },
        );
    }

    #[test]
    fn define_list_produces_define_expression() {
        let bump = Bump::new();
        assert_eq!(
            must_parse("(define x 42)", &bump, &mut Heap::default()),
            Ast {
                span: 0..13,
                kind: AstKind::Define {
                    symbol: Symbol::must_inline("x"),
                    value: &Ast {
                        span: 10..12,
                        kind: AstKind::Constant(Val::Int(42)),
                    },
                },
            },
        );
    }

    #[test]
    fn define_lambda_syntax_produces_define_with_lambda_expression() {
        let bump = Bump::new();
        assert_eq!(
            must_parse("(define (add x y) (+ x y))", &bump, &mut Heap::default()),
            Ast {
                span: 0..26,
                kind: AstKind::Define {
                    symbol: Symbol::must_inline("add"),
                    value: &Ast {
                        span: 8..26,
                        kind: AstKind::Lambda {
                            args: &[Symbol::must_inline("x"), Symbol::must_inline("y")],
                            body: &Ast {
                                span: 18..25,
                                kind: AstKind::FunctionCall {
                                    function: &Ast {
                                        span: 19..20,
                                        kind: AstKind::Lookup {
                                            namespace: None,
                                            symbol: Symbol::must_inline("+")
                                        },
                                    },
                                    args: &[
                                        Ast {
                                            span: 21..22,
                                            kind: AstKind::Lookup {
                                                namespace: None,
                                                symbol: Symbol::must_inline("x")
                                            },
                                        },
                                        Ast {
                                            span: 23..24,
                                            kind: AstKind::Lookup {
                                                namespace: None,
                                                symbol: Symbol::must_inline("y")
                                            },
                                        },
                                    ],
                                },
                            },
                        },
                    },
                },
            },
        );
    }

    #[test]
    fn define_lambda_syntax_accepts_multiple_body_expressions() {
        let bump = Bump::new();

        let ast = must_parse("(define (f) 1 2)", &bump, &mut Heap::default());
        assert_matches!(
            ast.kind,
            AstKind::Define {
                symbol,
                value,
            } if symbol == Symbol::must_inline("f") && matches!(
                &value.kind,
                AstKind::Lambda { args, body }
                    if args.is_empty()
                        && matches!(&body.kind, AstKind::Begin(exprs) if exprs.len() == 2)
            )
        );
    }

    #[test]
    fn invalid_define_arity_reports_form_error_with_span() {
        assert_eq!(
            must_error("(define x)").as_slice(),
            &[AstError {
                kind: AstErrorKind::Form(FormErrorKind::DefineMissingValueOrFunctionBody),
                span: 0..10,
            }],
        );
        assert_eq!(
            must_error("(define x 1 2)").as_slice(),
            &[AstError {
                kind: AstErrorKind::Form(FormErrorKind::DefineTooManyArguments(3)),
                span: 0..14,
            }],
        );
    }

    #[test]
    fn define_requires_a_symbol() {
        assert_eq!(
            must_error("(define 1 42)").as_slice(),
            &[AstError {
                kind: AstErrorKind::Form(FormErrorKind::DefineInvalidName),
                span: 8..9,
            }],
        );
    }

    #[test]
    fn set_requires_exactly_two_arguments() {
        for (source, kind) in [
            ("(set!)", FormErrorKind::SetMissingNameAndValue),
            ("(set! x)", FormErrorKind::SetMissingValue),
            ("(set! x 1 2)", FormErrorKind::SetTooManyArguments(3)),
        ] {
            assert_eq!(
                must_error(source).as_slice(),
                &[AstError {
                    kind: AstErrorKind::Form(kind),
                    span: 0..source.len(),
                }],
            );
        }
    }

    #[test]
    fn set_requires_a_symbol_name() {
        for (source, span) in [("(set! 1 42)", 6..7), ("(set! (f x) 42)", 6..11)] {
            assert_eq!(
                must_error(source).as_slice(),
                &[AstError {
                    kind: AstErrorKind::Form(FormErrorKind::SetInvalidName),
                    span,
                }],
            );
        }
    }

    #[test]
    fn set_produces_assignment_expression() {
        let bump = Bump::new();
        let ast = must_parse("(set! x 42)", &bump, &mut Heap::default());
        assert_eq!(
            ast,
            Ast {
                kind: AstKind::Set {
                    symbol: Symbol::must_inline("x"),
                    value: &Ast {
                        kind: AstKind::Constant(Val::Int(42)),
                        span: 8..10,
                    },
                },
                span: 0..11,
            },
        );
    }

    #[test]
    fn lambda_list_produces_lambda_expression() {
        let bump = Bump::new();
        let ast = must_parse("(lambda (x y) (+ x y))", &bump, &mut Heap::default());

        assert_eq!(
            ast,
            Ast {
                span: 0..22,
                kind: AstKind::Lambda {
                    args: &[Symbol::must_inline("x"), Symbol::must_inline("y")],
                    body: &Ast {
                        span: 14..21,
                        kind: AstKind::FunctionCall {
                            function: &Ast {
                                span: 15..16,
                                kind: AstKind::Lookup {
                                    namespace: None,
                                    symbol: Symbol::must_inline("+")
                                },
                            },
                            args: &[
                                Ast {
                                    span: 17..18,
                                    kind: AstKind::Lookup {
                                        namespace: None,
                                        symbol: Symbol::must_inline("x")
                                    },
                                },
                                Ast {
                                    span: 19..20,
                                    kind: AstKind::Lookup {
                                        namespace: None,
                                        symbol: Symbol::must_inline("y")
                                    },
                                },
                            ],
                        },
                    },
                },
            },
        );
    }

    #[test]
    fn lambda_accepts_empty_parameters_and_multiple_body_expressions() {
        let bump = Bump::new();
        let ast = must_parse("(lambda () 1 2)", &bump, &mut Heap::default());

        assert_eq!(
            ast,
            Ast {
                span: 0..15,
                kind: AstKind::Lambda {
                    args: &[],
                    body: &Ast {
                        span: 11..14,
                        kind: AstKind::Begin(&[
                            Ast {
                                span: 11..12,
                                kind: AstKind::Constant(Val::Int(1)),
                            },
                            Ast {
                                span: 13..14,
                                kind: AstKind::Constant(Val::Int(2)),
                            },
                        ]),
                    },
                },
            },
        );
    }

    #[test]
    fn lambda_parameters_must_be_symbols() {
        assert_eq!(
            must_error("(lambda (1) 42)").as_slice(),
            &[AstError {
                kind: AstErrorKind::Form(FormErrorKind::LambdaInvalidParameter),
                span: 9..10,
            }],
        );
    }

    #[test]
    fn lambda_reports_all_invalid_parameters() {
        assert_eq!(
            must_error("(lambda (1 2 3) 42)").as_slice(),
            &[
                AstError {
                    kind: AstErrorKind::Form(FormErrorKind::LambdaInvalidParameter),
                    span: 9..10,
                },
                AstError {
                    kind: AstErrorKind::Form(FormErrorKind::LambdaInvalidParameter),
                    span: 11..12,
                },
                AstError {
                    kind: AstErrorKind::Form(FormErrorKind::LambdaInvalidParameter),
                    span: 13..14,
                },
            ],
        );
    }

    #[test]
    fn lambda_requires_parameters_and_body() {
        assert_eq!(
            must_error("(lambda)").as_slice(),
            &[AstError {
                kind: AstErrorKind::Form(FormErrorKind::LambdaMissingParameterListAndBody),
                span: 0..8,
            }],
        );
        assert_eq!(
            must_error("(lambda ())").as_slice(),
            &[AstError {
                kind: AstErrorKind::Form(FormErrorKind::LambdaMissingBody),
                span: 0..11,
            }],
        );
    }

    #[test]
    fn invalid_return_arity_reports_form_error_with_span() {
        assert_eq!(
            must_error("(return 1 2)").as_slice(),
            &[AstError {
                kind: AstErrorKind::Form(FormErrorKind::ReturnTooManyArguments(2)),
                span: 0..12,
            }],
        );
    }

    #[test]
    fn call_in_function_position_preserves_nested_function() {
        let bump = Bump::new();
        assert_eq!(
            must_parse("((foo) 1)", &bump, &mut Heap::default()),
            Ast {
                span: 0..9,
                kind: AstKind::FunctionCall {
                    function: &Ast {
                        span: 1..6,
                        kind: AstKind::FunctionCall {
                            function: &Ast {
                                span: 2..5,
                                kind: AstKind::Lookup {
                                    namespace: None,
                                    symbol: Symbol::must_inline("foo")
                                },
                            },
                            args: &[],
                        },
                    },
                    args: &[Ast {
                        span: 7..8,
                        kind: AstKind::Constant(Val::Int(1))
                    }],
                },
            },
        );
    }

    #[test]
    fn multiple_top_level_forms_produce_begin_expression() {
        let bump = Bump::new();
        assert_eq!(
            must_parse("1 2", &bump, &mut Heap::default()),
            Ast {
                kind: AstKind::Begin(&[
                    Ast {
                        kind: AstKind::Constant(Val::Int(1)),
                        span: 0..1,
                    },
                    Ast {
                        kind: AstKind::Constant(Val::Int(2)),
                        span: 2..3,
                    },
                ]),
                span: 0..3,
            },
        );
        assert_eq!(
            must_parse("1 (foo)", &bump, &mut Heap::default()),
            Ast {
                kind: AstKind::Begin(&[
                    Ast {
                        kind: AstKind::Constant(Val::Int(1)),
                        span: 0..1,
                    },
                    Ast {
                        kind: AstKind::FunctionCall {
                            function: &Ast {
                                kind: AstKind::Lookup {
                                    namespace: None,
                                    symbol: Symbol::must_inline("foo")
                                },
                                span: 3..6,
                            },
                            args: &[],
                        },
                        span: 2..7,
                    },
                ]),
                span: 0..7,
            },
        );
    }

    #[test]
    fn empty_list_reports_empty_expression_with_span() {
        assert_eq!(
            must_error(" ( ) ").as_slice(),
            &[AstError {
                kind: AstErrorKind::Form(FormErrorKind::EmptyExpression),
                span: 1..4,
            }],
        );
    }

    #[test]
    fn invalid_if_arity_reports_form_error_with_span() {
        assert_eq!(
            must_error(" (if 1) ").as_slice(),
            &[AstError {
                kind: AstErrorKind::Form(FormErrorKind::IfMissingBranches),
                span: 1..7,
            }],
        );
        assert_eq!(
            must_error("(if 1 2 3 4)").as_slice(),
            &[AstError {
                kind: AstErrorKind::Form(FormErrorKind::IfTooManyArguments(4)),
                span: 0..12,
            }],
        );
    }

    #[test]
    fn nested_empty_lists_report_each_invalid_expression() {
        assert_eq!(
            must_error("(foo () (bar ()))").as_slice(),
            &[
                AstError {
                    kind: AstErrorKind::Form(FormErrorKind::EmptyExpression),
                    span: 5..7,
                },
                AstError {
                    kind: AstErrorKind::Form(FormErrorKind::EmptyExpression),
                    span: 13..15,
                },
            ],
        );
    }

    #[test]
    fn empty_lists_are_only_allowed_as_lambda_parameters_or_let_bindings() {
        let bump = Bump::new();
        let mut heap = Heap::default();

        assert_eq!(
            must_parse("(lambda () 1)", &bump, &mut heap).kind,
            AstKind::Lambda {
                args: &[],
                body: &Ast {
                    kind: AstKind::Constant(Val::Int(1)),
                    span: 11..12,
                },
            }
        );
        assert_eq!(
            must_parse("(let () 1)", &bump, &mut heap).kind,
            AstKind::Let {
                bindings: &[],
                body: &Ast {
                    kind: AstKind::Constant(Val::Int(1)),
                    span: 8..9,
                },
            }
        );

        assert!(matches!(
            must_error("(lambda (x ()) 1)").as_slice(),
            [
                AstError {
                    kind: AstErrorKind::Form(FormErrorKind::EmptyExpression),
                    ..
                },
                AstError {
                    kind: AstErrorKind::Form(FormErrorKind::LambdaInvalidParameter),
                    ..
                }
            ]
        ));
        assert!(matches!(
            must_error("(let (()) 1)").as_slice(),
            [
                AstError {
                    kind: AstErrorKind::Form(FormErrorKind::EmptyExpression),
                    ..
                },
                AstError {
                    kind: AstErrorKind::Form(FormErrorKind::LetInvalidBinding),
                    ..
                }
            ]
        ));
    }

    #[test]
    fn let_requires_a_body_and_symbol_bindings() {
        assert!(matches!(
            must_error("(let ())").as_slice(),
            [AstError {
                kind: AstErrorKind::Form(FormErrorKind::LetMissingBody),
                ..
            }]
        ));
        assert!(matches!(
            must_error("(let ((1 2)) 1)").as_slice(),
            [AstError {
                kind: AstErrorKind::Form(FormErrorKind::LetInvalidBindingName),
                ..
            }]
        ));
    }

    #[test]
    fn let_requires_a_binding_list() {
        assert!(matches!(
            must_error("(let args 23)").as_slice(),
            [AstError {
                kind: AstErrorKind::Form(FormErrorKind::LetInvalidBindingList),
                ..
            }]
        ));
    }

    #[test]
    fn let_binding_reports_its_own_arity() {
        let errors = must_error("(let ((f 1 2)) nil)");
        assert!(matches!(
            errors.as_slice(),
            [AstError {
                kind: AstErrorKind::Form(FormErrorKind::InvalidBindingArity(3)),
                span: _,
            }]
        ));
        assert_eq!(errors.as_slice()[0].span, 6..13);
    }
}
