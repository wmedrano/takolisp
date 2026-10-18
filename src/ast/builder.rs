use bumpalo::{Bump, collections::Vec as BumpVec};

use crate::heap::Heap;
use crate::reader::Atom;
use crate::source_map::SourceId;
use crate::val::Val;
use crate::{
    ast::{Ast, AstKind, LetBinding},
    reader::{ReadErrorKind, ReadVisitor},
    symbol::Symbol,
    tokenizer::Span,
};

/// Builds an [`Ast`] from the events emitted by a [`crate::reader::Reader`].
///
/// Building happens in two phases. While reading, the source structure is recorded as a plain
/// tree of [`Sexp`] values. As soon as a top-level form is complete, that tree is interpreted as
/// an expression and turned into an [`Ast`].
pub struct AstBuilder<'bump, 'heap, 'src> {
    bump: &'bump Bump,
    heap: &'heap mut Heap,
    errors: AstErrors,
    /// Lists that have been opened but not yet closed, innermost last.
    open_lists: Vec<OpenList<'src>>,
    /// Top-level expressions that have been fully built.
    forms: Vec<Ast<'bump>>,
}

/// A piece of source as it was read, before it is interpreted as an expression.
struct Sexp<'src> {
    kind: SexpKind<'src>,
    span: Span,
}

enum SexpKind<'src> {
    Constant(Val),
    /// A symbol name, borrowed from the source.
    Symbol(&'src str),
    List(Vec<Sexp<'src>>),
}

/// A list whose closing paren has not been read yet.
struct OpenList<'src> {
    start: usize,
    items: Vec<Sexp<'src>>,
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

impl<'bump, 'heap, 'src> AstBuilder<'bump, 'heap, 'src> {
    pub fn new(source: SourceId, bump: &'bump Bump, heap: &'heap mut Heap) -> Self {
        AstBuilder {
            bump,
            heap,
            errors: AstErrors::new(source),
            open_lists: Vec::new(),
            forms: Vec::new(),
        }
    }

    /// Record an error found while reading the source.
    ///
    /// Any partially read lists are discarded, but the complete items inside of them are still
    /// checked so that form errors that came before the read error are reported.
    pub fn push_error(&mut self, kind: AstErrorKind, span: Span) {
        self.abandon_open_lists();
        self.errors.push(kind, span);
    }

    /// Finish building and return the `Ast` for the whole source.
    ///
    /// Multiple top-level forms are wrapped in a `begin` that spans `source_span`.
    pub fn into_ast(mut self, source_span: Span) -> Result<Ast<'bump>, AstErrors> {
        if !self.errors.as_slice().is_empty() {
            return Err(self.errors);
        }
        match self.forms.len() {
            0 => {
                self.errors
                    .push(AstErrorKind::Form(FormErrorKind::NoExpression), source_span);
                Err(self.errors)
            }
            1 => Ok(self.forms.pop().unwrap()),
            _ => Ok(Ast {
                kind: AstKind::Begin(self.bump.alloc_slice_fill_iter(self.forms)),
                span: source_span,
            }),
        }
    }
}

impl<'src> ReadVisitor<'src> for AstBuilder<'_, '_, 'src> {
    fn visit_atom(&mut self, span: Span, atom: Atom<'src>) {
        let kind = match atom {
            Atom::Symbol(name) => SexpKind::Symbol(name),
            atom => SexpKind::Constant(atom.to_val(self.heap)),
        };
        self.push_sexp(Sexp { kind, span });
    }

    fn start_list(&mut self, span: Span) {
        self.open_lists.push(OpenList {
            start: span.start,
            items: Vec::new(),
        });
    }

    fn end_list(&mut self, span: Span) {
        // The reader only ends lists it has started.
        let Some(list) = self.open_lists.pop() else {
            return;
        };
        self.push_sexp(Sexp {
            kind: SexpKind::List(list.items),
            span: list.start..span.end,
        });
    }

    fn visit_comment(&mut self, _span: Span, _comment: &str) {}
}

impl<'bump, 'src> AstBuilder<'bump, '_, 'src> {
    /// Add a complete sexp to the innermost open list, or build it if it is a top-level form.
    fn push_sexp(&mut self, sexp: Sexp<'src>) {
        match self.open_lists.last_mut() {
            Some(list) => list.items.push(sexp),
            None => {
                let ast = self.sexp_to_ast(&sexp);
                self.forms.push(ast);
            }
        }
    }

    /// Discard every unfinished list, checking only the complete items inside of them.
    ///
    /// The unfinished lists themselves are not checked since they would mostly report errors
    /// about whatever was cut off.
    fn abandon_open_lists(&mut self) {
        // Outer lists come first and an inner open list is always the last item of its parent,
        // so this visits items in source order.
        for list in std::mem::take(&mut self.open_lists) {
            for item in &list.items {
                self.sexp_to_ast(item);
            }
        }
    }

    fn sexp_to_ast(&mut self, sexp: &Sexp<'src>) -> Ast<'bump> {
        let span = sexp.span.clone();
        match &sexp.kind {
            SexpKind::Constant(val) => Ast {
                kind: AstKind::Constant(*val),
                span,
            },
            SexpKind::Symbol(name) => self.lookup(name, span),
            SexpKind::List(items) => self.list(items, span),
        }
    }

    /// Build each sexp as an expression.
    fn sexps_to_asts(&mut self, sexps: &[Sexp<'src>]) -> &'bump [Ast<'bump>] {
        let bump = self.bump;
        let mut asts = BumpVec::with_capacity_in(sexps.len(), bump);
        for sexp in sexps {
            asts.push(self.sexp_to_ast(sexp));
        }
        asts.into_bump_slice()
    }

    /// A symbol, optionally qualified by a namespace, as in `namespace/name`.
    fn lookup(&mut self, name: &str, span: Span) -> Ast<'bump> {
        let (namespace, symbol) = split_namespace(name);
        if symbol.is_empty() {
            return self.error(FormErrorKind::EmptySymbolName, span);
        }
        let namespace = namespace.map(|namespace| self.heap.symbols.create_symbol(namespace));
        let symbol = self.heap.symbols.create_symbol(symbol);
        Ast {
            kind: AstKind::Lookup { namespace, symbol },
            span,
        }
    }

    /// A special form, or a function call if the list does not start with a special form name.
    fn list(&mut self, items: &[Sexp<'src>], span: Span) -> Ast<'bump> {
        let Some((head, operands)) = items.split_first() else {
            return self.error(FormErrorKind::EmptyExpression, span);
        };
        if let SexpKind::Symbol(head_name) = head.kind {
            match head_name {
                "begin" => return self.begin(operands, span),
                "if" => return self.if_form(operands, span),
                "when" => return self.when(operands, span),
                "return" => return self.return_form(operands, span),
                "define" => return self.define(operands, span),
                "set!" => return self.set(operands, span),
                "lambda" => return self.lambda(operands, span),
                "let" => return self.let_form(operands, span),
                _ => {}
            }
        }
        self.call(head, operands, span)
    }

    /// `(function arg...)`
    fn call(&mut self, function: &Sexp<'src>, args: &[Sexp<'src>], span: Span) -> Ast<'bump> {
        let function = self.sexp_to_ast(function);
        let args = self.sexps_to_asts(args);
        Ast {
            kind: AstKind::FunctionCall {
                function: self.bump.alloc(function),
                args,
            },
            span,
        }
    }

    /// `(begin expr...)`
    fn begin(&mut self, operands: &[Sexp<'src>], span: Span) -> Ast<'bump> {
        Ast {
            kind: AstKind::Begin(self.sexps_to_asts(operands)),
            span,
        }
    }

    /// `(if condition then-branch [else-branch])`
    fn if_form(&mut self, operands: &[Sexp<'src>], span: Span) -> Ast<'bump> {
        let (condition, then_branch, else_branch) = match operands {
            [] => return self.error(FormErrorKind::IfMissingConditionAndBranches, span),
            [_] => return self.error(FormErrorKind::IfMissingBranches, span),
            [condition, then_branch] => (
                self.sexp_to_ast(condition),
                self.sexp_to_ast(then_branch),
                undefined(span.clone()),
            ),
            [condition, then_branch, else_branch] => (
                self.sexp_to_ast(condition),
                self.sexp_to_ast(then_branch),
                self.sexp_to_ast(else_branch),
            ),
            _ => {
                return self.error(FormErrorKind::IfTooManyArguments(operands.len()), span);
            }
        };
        Ast {
            kind: AstKind::If {
                condition: self.bump.alloc(condition),
                then_branch: self.bump.alloc(then_branch),
                else_branch: self.bump.alloc(else_branch),
            },
            span,
        }
    }

    /// `(when condition body...)`, an `if` without an else branch.
    fn when(&mut self, operands: &[Sexp<'src>], span: Span) -> Ast<'bump> {
        let [condition, body @ ..] = operands else {
            return self.error(FormErrorKind::WhenMissingConditionAndBody, span);
        };
        let condition = self.sexp_to_ast(condition);
        let then_branch = self.body(body, span.clone());
        Ast {
            kind: AstKind::If {
                condition: self.bump.alloc(condition),
                then_branch: self.bump.alloc(then_branch),
                else_branch: self.bump.alloc(undefined(span.clone())),
            },
            span,
        }
    }

    /// `(return [value])`
    fn return_form(&mut self, operands: &[Sexp<'src>], span: Span) -> Ast<'bump> {
        let value = match operands {
            [] => undefined(span.clone()),
            [value] => self.sexp_to_ast(value),
            _ => {
                return self.error(FormErrorKind::ReturnTooManyArguments(operands.len()), span);
            }
        };
        Ast {
            kind: AstKind::Return(self.bump.alloc(value)),
            span,
        }
    }

    /// `(define name value)` or `(define (name parameter...) body...)`
    fn define(&mut self, operands: &[Sexp<'src>], span: Span) -> Ast<'bump> {
        match operands {
            [] => self.error(
                FormErrorKind::DefineMissingNameAndValueOrFunctionDefinition,
                span,
            ),
            [_] => self.error(FormErrorKind::DefineMissingValueOrFunctionBody, span),
            [
                Sexp {
                    kind: SexpKind::List(signature),
                    span: signature_span,
                },
                body @ ..,
            ] => self.define_function(signature, signature_span, body, span),
            [name, value] => {
                let symbol = self.binding_name(name, FormErrorKind::DefineInvalidName);
                let value = self.sexp_to_ast(value);
                Ast {
                    kind: AstKind::Define {
                        symbol,
                        value: self.bump.alloc(value),
                    },
                    span,
                }
            }
            _ => self.error(FormErrorKind::DefineTooManyArguments(operands.len()), span),
        }
    }

    /// `(define (name parameter...) body...)`, which defines `name` as a lambda. The lambda
    /// spans from the signature to the end of the `define`.
    fn define_function(
        &mut self,
        signature: &[Sexp<'src>],
        signature_span: &Span,
        body: &[Sexp<'src>],
        span: Span,
    ) -> Ast<'bump> {
        let Some((name, parameters)) = signature.split_first() else {
            return self.error(FormErrorKind::DefineInvalidName, signature_span.clone());
        };
        let symbol = self.binding_name(name, FormErrorKind::DefineInvalidName);
        let lambda = self.function(parameters, body, signature_span.start..span.end);
        Ast {
            kind: AstKind::Define {
                symbol,
                value: self.bump.alloc(lambda),
            },
            span,
        }
    }

    /// `(set! name value)`
    fn set(&mut self, operands: &[Sexp<'src>], span: Span) -> Ast<'bump> {
        let (name, value) = match operands {
            [] => return self.error(FormErrorKind::SetMissingNameAndValue, span),
            [_] => return self.error(FormErrorKind::SetMissingValue, span),
            [name, value] => (name, value),
            _ => return self.error(FormErrorKind::SetTooManyArguments(operands.len()), span),
        };
        let symbol = self.binding_name(name, FormErrorKind::SetInvalidName);
        let value = self.sexp_to_ast(value);
        Ast {
            kind: AstKind::Set {
                symbol,
                value: self.bump.alloc(value),
            },
            span,
        }
    }

    /// `(lambda (parameter...) body...)`
    fn lambda(&mut self, operands: &[Sexp<'src>], span: Span) -> Ast<'bump> {
        match operands {
            [] => self.error(FormErrorKind::LambdaMissingParameterListAndBody, span),
            [_] => self.error(FormErrorKind::LambdaMissingBody, span),
            [parameters, body @ ..] => {
                let SexpKind::List(parameters) = &parameters.kind else {
                    return self.error(
                        FormErrorKind::LambdaInvalidParameter,
                        parameters.span.clone(),
                    );
                };
                self.function(parameters, body, span)
            }
        }
    }

    /// A lambda built from its parameters and body.
    fn function(
        &mut self,
        parameters: &[Sexp<'src>],
        body: &[Sexp<'src>],
        span: Span,
    ) -> Ast<'bump> {
        let bump = self.bump;
        let args = BumpVec::from_iter_in(
            parameters.iter().map(|parameter| {
                self.binding_name(parameter, FormErrorKind::LambdaInvalidParameter)
            }),
            bump,
        )
        .into_bump_slice();
        let body = self.body(body, span.clone());
        Ast {
            kind: AstKind::Lambda {
                args,
                body: bump.alloc(body),
            },
            span,
        }
    }

    /// `(let ((name value)...) body...)`
    fn let_form(&mut self, operands: &[Sexp<'src>], span: Span) -> Ast<'bump> {
        let (bindings, body) = match operands {
            [] => return self.error(FormErrorKind::LetMissingBindingListAndBody, span),
            [_] => return self.error(FormErrorKind::LetMissingBody, span),
            [bindings, body @ ..] => (bindings, body),
        };
        let SexpKind::List(bindings) = &bindings.kind else {
            return self.error(FormErrorKind::LetInvalidBindingList, bindings.span.clone());
        };
        let bump = self.bump;
        let bindings = BumpVec::from_iter_in(
            bindings.iter().map(|binding| self.let_binding(binding)),
            bump,
        )
        .into_bump_slice();
        let body = self.body(body, span.clone());
        Ast {
            kind: AstKind::Let {
                bindings,
                body: bump.alloc(body),
            },
            span,
        }
    }

    /// A single `(name value)` binding of a `let` form.
    fn let_binding(&mut self, binding: &Sexp<'src>) -> LetBinding<'bump> {
        let (symbol, value) = match &binding.kind {
            SexpKind::List(items) => match items.as_slice() {
                [name, value] => (
                    self.binding_name(name, FormErrorKind::LetInvalidBindingName),
                    self.sexp_to_ast(value),
                ),
                [] => (
                    Symbol::UNDERSCORE,
                    self.invalid(binding, FormErrorKind::LetInvalidBinding),
                ),
                _ => (
                    Symbol::UNDERSCORE,
                    self.error(
                        FormErrorKind::InvalidBindingArity(items.len()),
                        binding.span.clone(),
                    ),
                ),
            },
            _ => (
                Symbol::UNDERSCORE,
                self.invalid(binding, FormErrorKind::LetInvalidBinding),
            ),
        };
        LetBinding {
            symbol,
            value: self.bump.alloc(value),
        }
    }

    /// The body of a `lambda`, `let`, or `when`.
    ///
    /// An empty body evaluates to undefined and several expressions are wrapped in a `begin`.
    fn body(&mut self, exprs: &[Sexp<'src>], form_span: Span) -> Ast<'bump> {
        match exprs {
            [] => undefined(form_span),
            [expr] => self.sexp_to_ast(expr),
            [first, .., last] => Ast {
                kind: AstKind::Begin(self.sexps_to_asts(exprs)),
                span: first.span.start..last.span.end,
            },
        }
    }

    /// A name being bound by a special form. Names must be unqualified symbols.
    fn binding_name(&mut self, sexp: &Sexp<'src>, error: FormErrorKind) -> Symbol {
        match sexp.kind {
            SexpKind::Symbol(name) if split_namespace(name).0.is_none() => {
                self.heap.symbols.create_symbol(name)
            }
            _ => {
                self.invalid(sexp, error);
                Symbol::UNDERSCORE
            }
        }
    }

    /// Record `error` for a sexp that does not belong where it was found.
    ///
    /// The sexp is still built as an expression first so that errors nested inside of it are
    /// reported too.
    fn invalid(&mut self, sexp: &Sexp<'src>, error: FormErrorKind) -> Ast<'bump> {
        self.sexp_to_ast(sexp);
        self.error(error, sexp.span.clone())
    }

    /// Record a form error and return a placeholder expression in its place.
    fn error(&mut self, kind: FormErrorKind, span: Span) -> Ast<'bump> {
        self.errors.push(AstErrorKind::Form(kind), span.clone());
        undefined(span)
    }
}

/// An expression that evaluates to undefined.
fn undefined<'a>(span: Span) -> Ast<'a> {
    Ast {
        kind: AstKind::Constant(Val::Undefined),
        span,
    }
}

/// Split `name` at its final `/` into an optional namespace and a symbol name.
///
/// A lone `/` is the symbol `/` rather than a separator.
fn split_namespace(name: &str) -> (Option<&str>, &str) {
    match name.rsplit_once('/') {
        Some((namespace, symbol)) if name != "/" => (Some(namespace), symbol),
        _ => (None, name),
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
