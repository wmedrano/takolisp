//! Errors produced while parsing, compiling, or evaluating takolisp code.

use std::path::PathBuf;

use crate::Vm;
use crate::ast::AstErrors;
use crate::ast::{AstError, AstErrorKind, FormErrorKind};
use crate::function::Arity;
use crate::reader::ReadErrorKind;
use crate::{source_map::SourceId, symbol::Symbol, tokenizer::Span, val::Val};

/// An error encountered while compiling an AST.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompilerError {
    /// The kind of error that occurred.
    pub kind: CompilerErrorKind,
    /// The source span associated with the compiler operation.
    pub span: Span,
}

/// The kind of error encountered while compiling an AST.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompilerErrorKind {
    /// An invariant violation encountered while compiling valid AST data.
    Internal(&'static str),
    /// A symbol was referenced without a definition in the compilation module
    /// or the VM's global module.
    UnboundSymbol(Symbol),
    /// An assignment targeted an immutable function name or global value.
    ImmutableSymbol(Symbol),
    /// A lookup used a namespace, which is not yet supported.
    UnsupportedNamespace(Symbol),
}

impl CompilerError {
    pub(crate) fn new(message: &'static str, span: Span) -> Self {
        Self {
            kind: CompilerErrorKind::Internal(message),
            span,
        }
    }

    /// Creates an error for a reference to a symbol without a definition.
    pub(crate) fn unbound_symbol(symbol: Symbol, span: Span) -> Self {
        Self {
            kind: CompilerErrorKind::UnboundSymbol(symbol),
            span,
        }
    }

    pub(crate) fn immutable_symbol(symbol: Symbol, span: Span) -> Self {
        Self {
            kind: CompilerErrorKind::ImmutableSymbol(symbol),
            span,
        }
    }
}

impl std::fmt::Display for CompilerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            CompilerErrorKind::Internal(message) => write!(
                f,
                "internal compiler error: {}; please report this bug",
                message
            ),
            CompilerErrorKind::UnboundSymbol(_) => f.write_str("unbound symbol"),
            CompilerErrorKind::ImmutableSymbol(_) => {
                f.write_str("cannot assign to immutable symbol")
            }
            CompilerErrorKind::UnsupportedNamespace(_) => {
                f.write_str("namespace lookups are not yet supported")
            }
        }
    }
}

/// Colors used when formatting a [`VmError`].
#[derive(Copy, Clone, Debug, Default)]
pub struct ErrorPalette {
    /// Color for the error heading and source marker.
    pub error: &'static str,
    /// Color for the source location.
    pub location: &'static str,
    /// Sequence that resets the terminal color.
    pub reset: &'static str,
}

/// Options for [`VmError::display`].
#[derive(Copy, Clone, Debug, Default)]
pub struct VmErrorConfig {
    /// An optional color palette. Formatting is plain text when absent.
    pub palette: ErrorPalette,
}

/// An error that occurs at runtime.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum RuntimeError {
    /// A value had a different type than the operation expected.
    ///
    /// Contains the expected type name and the value that was found.
    TypeError {
        /// The type required by the operation.
        expected: &'static str,
        /// The value that had the wrong type.
        found: Val,
    },
    /// A symbol or module slot was referenced without a corresponding value.
    ValueNotFound(Option<Symbol>),
    /// A function was called with the wrong number of arguments.
    ArityError {
        /// The argument count accepted by the function.
        expected: Arity,
        /// The number of arguments supplied to the function.
        found: usize,
    },
    /// An operation attempted to access an empty stack.
    StackUnderflow,
    /// An operation attempted to push past the stack.
    StackOverflow,
    /// An integer operation produced a value outside the range of a 64-bit integer.
    Overflow,
    /// Bytecode violated the VM's operand-stack contract.
    InvalidBytecode(&'static str),
    /// Internal VM state violated a call-frame invariant.
    InvalidVmState(&'static str),
}

/// An error that occurs while evaluating source code.
#[derive(Clone, Debug, PartialEq)]
pub struct VmError {
    /// The kind of error that occurred.
    pub(crate) kind: VmErrorKind,
}

impl VmError {
    /// Format this error with a useful message and, for source errors, a
    /// source excerpt pointing at the offending span. Use the VM that evaluated
    /// the source to include its source context.
    pub fn display<'b>(&'b self, vm: &'b Vm, config: VmErrorConfig) -> impl std::fmt::Display + 'b {
        DisplayVmError {
            error: self,
            vm,
            config,
        }
    }
}

struct DisplayVmError<'a> {
    error: &'a VmError,
    vm: &'a Vm,
    config: VmErrorConfig,
}

impl std::fmt::Display for DisplayVmError<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (color, location, reset) = (
            self.config.palette.error,
            self.config.palette.location,
            self.config.palette.reset,
        );
        match &self.error.kind {
            VmErrorKind::FileRead { path, error } => {
                writeln!(
                    f,
                    "{color}error:{reset} cannot read {}: {error}",
                    path.display()
                )
            }
            VmErrorKind::Runtime(error) => {
                let maybe_func = self.vm.call_frames.current().map(|frame| frame.function);
                let message = RuntimeMessage { error, vm: self.vm };
                match maybe_func {
                    None => writeln!(f, "{color}error:{reset} {message}")?,
                    Some(func) => writeln!(
                        f,
                        "{color}error:{reset} {message} in {func}",
                        func = func.display(self.vm)
                    )?,
                }
                writeln!(f, "\nbacktrace:")?;
                for (index, frame) in self.vm.call_frames.backtrace().enumerate() {
                    writeln!(f, "  {index}: {}", frame.function.display(self.vm),)?;
                }
                Ok(())
            }
            VmErrorKind::Compiler { error, source } => write_compiler_error(
                f,
                self.vm,
                error,
                self.vm.source_map.get(*source),
                color,
                location,
                reset,
            ),
            VmErrorKind::AstErrors { errors } => {
                for (index, error) in errors.as_slice().iter().enumerate() {
                    if index != 0 {
                        writeln!(f)?;
                    }
                    let source = self.vm.source_map.get(errors.source);
                    write_ast_error(f, error, source, color, location, reset)?;
                }
                Ok(())
            }
        }
    }
}

struct RuntimeMessage<'a> {
    error: &'a RuntimeError,
    vm: &'a Vm,
}

impl std::fmt::Display for RuntimeMessage<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.error {
            RuntimeError::TypeError { expected, found } => {
                write!(f, "expected {expected}, found {:?}", found.display(self.vm))
            }
            RuntimeError::ValueNotFound(Some(symbol)) => {
                write!(
                    f,
                    "value not found: {}",
                    self.vm.heap().symbols.display_symbol(*symbol)
                )
            }
            RuntimeError::ValueNotFound(None) => f.write_str("value not found"),
            RuntimeError::ArityError { expected, found } => match expected {
                Arity::Any => write!(f, "expected any number of arguments, found {found}"),
                Arity::Exact(expected) => write!(
                    f,
                    "expected {expected} {}, found {found} {}",
                    argument_word(*expected as usize),
                    argument_word(*found),
                ),
                Arity::AtLeast(expected) => {
                    write!(
                        f,
                        "expected at least {expected} {}, found {found} {}",
                        argument_word(*expected as usize),
                        argument_word(*found),
                    )
                }
            },
            RuntimeError::StackUnderflow => f.write_str("evaluation stack underflow"),
            RuntimeError::StackOverflow => f.write_str("evaluation stack overflow"),
            RuntimeError::Overflow => f.write_str("integer overflow"),
            RuntimeError::InvalidBytecode(message) => write!(f, "invalid bytecode: {message}"),
            RuntimeError::InvalidVmState(message) => write!(f, "invalid VM state: {message}"),
        }
    }
}

fn argument_word(count: usize) -> &'static str {
    if count == 1 { "argument" } else { "arguments" }
}

fn write_ast_error(
    f: &mut std::fmt::Formatter<'_>,
    error: &AstError,
    source: Option<&str>,
    color: &str,
    location_color: &str,
    reset: &str,
) -> std::fmt::Result {
    write!(f, "{color}error:{reset} ")?;
    match &error.kind {
        AstErrorKind::Read(ReadErrorKind::UnexpectedEndParen) => {
            f.write_str("unexpected closing parenthesis")?
        }
        AstErrorKind::Read(ReadErrorKind::MissingEndParen) => {
            f.write_str("missing closing parenthesis")?
        }
        AstErrorKind::Read(ReadErrorKind::MalformedString) => f.write_str("malformed string")?,
        AstErrorKind::Form(FormErrorKind::NoExpression) => f.write_str("no expression found")?,
        AstErrorKind::Form(FormErrorKind::EmptyExpression) => {
            f.write_str("empty expression; add an expression between the parentheses, e.g. (+ 1 2)")?
        }
        AstErrorKind::Form(FormErrorKind::EmptySymbolName) => f.write_str(
            "qualified symbol must have a name after the final '/', e.g. namespace/name",
        )?,
        AstErrorKind::Form(FormErrorKind::LambdaMissingParameterListAndBody) => f.write_str(
            "lambda expects a parameter list and a body, e.g. (lambda (x) (+ x 1))",
        )?,
        AstErrorKind::Form(FormErrorKind::LambdaMissingBody) => f.write_str(
            "lambda expects a body after its parameter list, e.g. (lambda () body)",
        )?,
        AstErrorKind::Form(FormErrorKind::LetMissingBindingListAndBody) => f.write_str(
            "let expects a binding list and a body, e.g. (let ((name value)) body)",
        )?,
        AstErrorKind::Form(FormErrorKind::LetMissingBody) => f.write_str(
            "let expects a body after the binding list, e.g. (let ((name value)) body)",
        )?,
        AstErrorKind::Form(FormErrorKind::IfMissingConditionAndBranches) => f.write_str(
            "if expects a condition, a true-case value, and optionally a false-case value, e.g. (if condition then-value else-value)",
        )?,
        AstErrorKind::Form(FormErrorKind::IfMissingBranches) => f.write_str(
            "if expects a true-case value after the condition, optionally followed by a false-case value, e.g. (if condition then-value else-value)",
        )?,
        AstErrorKind::Form(FormErrorKind::IfTooManyArguments(found)) => write!(
            f,
            "if expects 2 or 3 arguments: a condition, a true-case value, and optionally a false-case value; found {found}; e.g. (if condition then-value else-value)"
        )?,
        AstErrorKind::Form(FormErrorKind::WhenMissingConditionAndBody) => write!(
            f,
            "when expects a condition followed by one or more body expressions, e.g. (when condition body)"
        )?,
        AstErrorKind::Form(FormErrorKind::ReturnTooManyArguments(found)) => write!(
            f,
            "return expects zero or one value, found {found} arguments; e.g. (return) or (return value)"
        )?,
        AstErrorKind::Form(FormErrorKind::DefineMissingNameAndValueOrFunctionDefinition) => {
            f.write_str(
                "define expects a name and a value, or a function definition, e.g. (define name value) or (define (name arg) body)",
            )?
        }
        AstErrorKind::Form(FormErrorKind::DefineMissingValueOrFunctionBody) => f.write_str(
            "define expects a value after the name, e.g. (define name value), or a function body, e.g. (define (name arg) body)",
        )?,
        AstErrorKind::Form(FormErrorKind::DefineTooManyArguments(found)) => write!(
            f,
            "define expects a name/value pair or a function definition, found {found} arguments; e.g. (define name value) or (define (name arg) body)"
        )?,
        AstErrorKind::Form(FormErrorKind::SetMissingNameAndValue) => f.write_str(
            "set! expects a name and a value, e.g. (set! name value)",
        )?,
        AstErrorKind::Form(FormErrorKind::SetMissingValue) => f.write_str(
            "set! expects a value after the name, e.g. (set! name value)",
        )?,
        AstErrorKind::Form(FormErrorKind::SetTooManyArguments(found)) => write!(
            f,
            "set! expects exactly 2 arguments: a name and a value, found {found}; e.g. (set! name value)"
        )?,
        AstErrorKind::Form(FormErrorKind::SetInvalidName) => f.write_str(
            "set! expects an unqualified symbol name, e.g. (set! name value)",
        )?,
        AstErrorKind::Form(FormErrorKind::LambdaInvalidParameter) => f.write_str(
            "lambda expects a parameter list containing only unqualified symbols, e.g. (lambda (x y) (+ x y))",
        )?,
        AstErrorKind::Form(FormErrorKind::LetInvalidBindingName) => {
            f.write_str("let binding names must be unqualified symbols")?
        }
        AstErrorKind::Form(FormErrorKind::DefineInvalidName) => f.write_str(
            "define expects an unqualified symbol name, e.g. (define name value) or (define (name arg) body)",
        )?,
        AstErrorKind::Form(FormErrorKind::InvalidBindingArity(found)) => write!(
            f,
            "let binding expects exactly 2 elements: a name and a value, found {found}; e.g. (name value)"
        )?,
        AstErrorKind::Form(FormErrorKind::LetInvalidBindingList) => f.write_str(
            "let expects a binding list containing name/value pairs, e.g. (let ((name value)) body)",
        )?,
        AstErrorKind::Form(FormErrorKind::LetInvalidBinding) => f.write_str(
            "let expects each binding to be a name/value pair, e.g. (let ((name value)) body)",
        )?,
    }
    writeln!(f)?;
    write_span_context(f, source, error.span.clone(), color, location_color, reset)
}

fn write_compiler_error(
    f: &mut std::fmt::Formatter<'_>,
    vm: &Vm,
    error: &CompilerError,
    source: Option<&str>,
    color: &str,
    location_color: &str,
    reset: &str,
) -> std::fmt::Result {
    write!(f, "{color}error:{reset} ")?;
    match &error.kind {
        CompilerErrorKind::Internal(message) => writeln!(
            f,
            "internal compiler error: {message}; please report this bug"
        )?,
        CompilerErrorKind::UnboundSymbol(symbol) => writeln!(
            f,
            "unbound symbol: {}",
            vm.heap().symbols.display_symbol(*symbol)
        )?,
        CompilerErrorKind::ImmutableSymbol(symbol) => writeln!(
            f,
            "cannot assign to immutable symbol: {}",
            vm.heap().symbols.display_symbol(*symbol)
        )?,
        CompilerErrorKind::UnsupportedNamespace(namespace) => writeln!(
            f,
            "namespace lookups are not yet supported: {}",
            vm.heap().symbols.display_symbol(*namespace)
        )?,
    }
    write_span_context(f, source, error.span.clone(), color, location_color, reset)
}

fn write_span_context(
    f: &mut std::fmt::Formatter<'_>,
    source: Option<&str>,
    span: Span,
    color: &str,
    location_color: &str,
    reset: &str,
) -> std::fmt::Result {
    let Some(source) = source else {
        return Ok(());
    };

    let (line, column) = line_column(source, span.start);
    let line_start = source[..span.start.min(source.len())]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let line_end = source[span.start.min(source.len())..]
        .find('\n')
        .map_or(source.len(), |index| span.start.min(source.len()) + index);
    let source_line = &source[line_start..line_end];
    let marker_column = span.start.saturating_sub(line_start);
    let marker_len = span.end.saturating_sub(span.start).max(1);

    writeln!(f, "{location_color} --> {line}:{column}{reset}")?;
    writeln!(f, "   |")?;
    writeln!(f, "{line:>2} | {source_line}")?;
    let marker = format!("{}{}", " ".repeat(marker_column), "^".repeat(marker_len));
    writeln!(f, "   | {color}{marker}{reset}")
}

fn line_column(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let prefix = &source[..offset];
    (
        prefix.bytes().filter(|byte| *byte == b'\n').count() + 1,
        prefix
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .chars()
            .count()
            + 1,
    )
}

/// The kind of error that occurs while evaluating source code.
#[derive(Clone, Debug, PartialEq)]
pub enum VmErrorKind {
    /// A file could not be read as source text.
    FileRead { path: PathBuf, error: String },
    /// An error occurred while parsing or building the AST.
    AstErrors { errors: AstErrors },
    /// An error occurred while evaluating the compiled code.
    Runtime(RuntimeError),
    /// An error occurred while compiling the AST.
    Compiler {
        error: CompilerError,
        source: SourceId,
    },
}

impl From<AstErrors> for VmError {
    fn from(errors: AstErrors) -> Self {
        Self {
            kind: VmErrorKind::AstErrors { errors },
        }
    }
}

impl From<RuntimeError> for VmError {
    fn from(error: RuntimeError) -> Self {
        Self {
            kind: VmErrorKind::Runtime(error),
        }
    }
}

impl From<CompilerError> for VmError {
    fn from(error: CompilerError) -> Self {
        Self {
            kind: VmErrorKind::Compiler {
                error,
                source: SourceId::new_unique(),
            },
        }
    }
}

impl VmError {
    pub(crate) fn from_file_read(path: &std::path::Path, error: std::io::Error) -> Self {
        Self {
            kind: VmErrorKind::FileRead {
                path: path.to_owned(),
                error: error.to_string(),
            },
        }
    }

    pub(crate) fn from_compiler(error: CompilerError, source: SourceId) -> Self {
        Self {
            kind: VmErrorKind::Compiler { error, source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_includes_source_context_for_parse_errors() {
        let mut vm = Vm::new();
        let error = vm
            .eval_str(vm.global_module(), "(")
            .expect_err("expected a parse error");

        assert_eq!(
            error.display(&vm, VmErrorConfig::default()).to_string(),
            "error: missing closing parenthesis\n --> 1:1\n   |\n 1 | (\n   | ^\n"
        );
    }

    #[test]
    fn parse_error_keeps_source_context_after_input_is_dropped_and_vm_is_reused() {
        let mut vm = Vm::new();
        let error = {
            let source = String::from("(");
            vm.eval_str(vm.global_module(), &source)
                .expect_err("expected a parse error")
        };
        vm.eval_str(vm.global_module(), "(+ 1 2)").unwrap();
        vm.eval_str(vm.global_module(), "(+ 3 4)").unwrap();

        assert_eq!(
            error.display(&vm, VmErrorConfig::default()).to_string(),
            "error: missing closing parenthesis\n --> 1:1\n   |\n 1 | (\n   | ^\n"
        );
    }

    #[test]
    fn display_handles_parse_errors_without_source_context() {
        let mut vm = Vm::new();
        let error = vm
            .eval_str(vm.global_module(), "(")
            .expect_err("expected a parse error");
        let VmErrorKind::AstErrors { errors, .. } = error.kind else {
            panic!("expected AST errors");
        };
        let error = VmError::from(errors);
        let vm = Vm::new();

        assert_eq!(
            error.display(&vm, VmErrorConfig::default()).to_string(),
            "error: missing closing parenthesis\n"
        );
    }

    #[test]
    fn display_includes_source_context_for_compiler_errors() {
        let mut vm = Vm::new();
        let source = vm.source_map.add_source("value");
        let error = VmError::from_compiler(CompilerError::new("test compiler error", 0..5), source);

        assert_eq!(
            error.display(&vm, VmErrorConfig::default()).to_string(),
            "error: internal compiler error: test compiler error; please report this bug\n"
                .to_owned()
                + " --> 1:1\n   |\n 1 | value\n   | ^^^^^\n"
        );
    }

    #[test]
    fn display_explains_invalid_lambda_parameters() {
        let mut vm = Vm::new();
        let error = vm
            .eval_str(vm.global_module(), "(lambda (1 2) 42)")
            .expect_err("expected a form error");

        let rendered = error.display(&vm, VmErrorConfig::default()).to_string();
        assert!(rendered.contains(
            "error: lambda expects a parameter list containing only unqualified symbols, e.g. (lambda (x y) (+ x y))\n"
        ));
        assert_eq!(
            rendered
                .matches("lambda expects a parameter list containing only unqualified symbols")
                .count(),
            2
        );
    }

    #[test]
    fn display_explains_invalid_let_binding_arity() {
        let mut vm = Vm::new();
        let error = vm
            .eval_str(vm.global_module(), "(let ((f 1 2)) nil)")
            .expect_err("expected a form error");

        assert_eq!(
            error.display(&vm, VmErrorConfig::default()).to_string(),
            concat!(
                "error: let binding expects exactly 2 elements: a name and a value, found 3; ",
                "e.g. (name value)\n",
                " --> 1:7\n",
                "   |\n",
                " 1 | (let ((f 1 2)) nil)\n",
                "   |       ^^^^^^^\n",
            )
        );
    }

    #[test]
    fn display_explains_invalid_if_arity() {
        let mut vm = Vm::new();

        let error = vm
            .eval_str(vm.global_module(), "(if true)")
            .expect_err("expected a form error");
        assert!(error.display(&vm, VmErrorConfig::default()).to_string().starts_with(
            "error: if expects a true-case value after the condition, optionally followed by a false-case value, e.g. (if condition then-value else-value)\n"
        ));

        let error = vm
            .eval_str(vm.global_module(), "(if true 1 2 3)")
            .expect_err("expected a form error");
        assert!(error.display(&vm, VmErrorConfig::default()).to_string().starts_with(
            "error: if expects 2 or 3 arguments: a condition, a true-case value, and optionally a false-case value; found 4; e.g. (if condition then-value else-value)\n"
        ));
    }

    #[test]
    fn display_explains_invalid_set_forms() {
        let mut vm = Vm::new();
        for (source, message, location, marker) in [
            (
                "(set!)",
                "set! expects a name and a value, e.g. (set! name value)",
                "1:1",
                "^^^^^^",
            ),
            (
                "(set! x)",
                "set! expects a value after the name, e.g. (set! name value)",
                "1:1",
                "^^^^^^^^",
            ),
            (
                "(set! x 1 2)",
                "set! expects exactly 2 arguments: a name and a value, found 3; e.g. (set! name value)",
                "1:1",
                "^^^^^^^^^^^^",
            ),
            (
                "(set! 1 2)",
                "set! expects an unqualified symbol name, e.g. (set! name value)",
                "1:7",
                "      ^",
            ),
        ] {
            let error = vm
                .eval_str(vm.global_module(), source)
                .expect_err("expected a form error");
            assert_eq!(
                error.display(&vm, VmErrorConfig::default()).to_string(),
                format!("error: {message}\n --> {location}\n   |\n 1 | {source}\n   | {marker}\n"),
            );
        }
    }

    #[test]
    fn display_explains_invalid_define_forms() {
        let mut vm = Vm::new();

        let error = vm
            .eval_str(vm.global_module(), "(define)")
            .expect_err("expected a form error");
        assert!(error.display(&vm, VmErrorConfig::default()).to_string().starts_with(
            "error: define expects a name and a value, or a function definition, e.g. (define name value) or (define (name arg) body)\n"
        ));

        let error = vm
            .eval_str(vm.global_module(), "(define 1 2)")
            .expect_err("expected a form error");
        assert!(error.display(&vm, VmErrorConfig::default()).to_string().starts_with(
            "error: define expects an unqualified symbol name, e.g. (define name value) or (define (name arg) body)\n"
        ));

        let error = vm
            .eval_str(vm.global_module(), "()")
            .expect_err("expected a form error");
        assert!(error.display(&vm, VmErrorConfig::default()).to_string().starts_with(
            "error: empty expression; add an expression between the parentheses, e.g. (+ 1 2)\n"
        ));
    }

    #[test]
    fn display_includes_source_context_for_unbound_symbols() {
        let mut vm = Vm::new();
        let error = vm
            .eval_str(vm.global_module(), "missing")
            .expect_err("expected a compile error");

        assert_eq!(
            error.display(&vm, VmErrorConfig::default()).to_string(),
            concat!(
                "error: unbound symbol: missing\n",
                " --> 1:1\n",
                "   |\n",
                " 1 | missing\n",
                "   | ^^^^^^^\n",
            )
        );
    }

    #[test]
    fn display_includes_source_context_for_immutable_symbols() {
        let mut vm = Vm::new();
        let source = vm.source_map.add_source("target");
        let compiler_error = CompilerError::immutable_symbol(Symbol::must_inline("target"), 0..6);
        assert_eq!(
            compiler_error.to_string(),
            "cannot assign to immutable symbol"
        );
        let error = VmError::from_compiler(compiler_error, source);

        assert_eq!(
            error.display(&vm, VmErrorConfig::default()).to_string(),
            concat!(
                "error: cannot assign to immutable symbol: target\n",
                " --> 1:1\n",
                "   |\n",
                " 1 | target\n",
                "   | ^^^^^^\n",
            )
        );
    }

    #[test]
    fn display_explains_empty_qualified_symbol_names() {
        let mut vm = Vm::new();
        let error = vm
            .eval_str(vm.global_module(), "a/b/c/")
            .expect_err("expected a parse error");
        assert!(matches!(error.kind, VmErrorKind::AstErrors { .. }));

        assert_eq!(
            error.display(&vm, VmErrorConfig::default()).to_string(),
            concat!(
                "error: qualified symbol must have a name after the final '/', e.g. namespace/name\n",
                " --> 1:1\n",
                "   |\n",
                " 1 | a/b/c/\n",
                "   | ^^^^^^\n",
            )
        );
    }

    #[test]
    fn display_includes_source_context_for_unsupported_namespaces() {
        let mut vm = Vm::new();
        let error = vm
            .eval_str(vm.global_module(), "long_namespace/x")
            .expect_err("expected an unsupported namespace error");

        assert_eq!(
            error.display(&vm, VmErrorConfig::default()).to_string(),
            concat!(
                "error: namespace lookups are not yet supported: long_namespace\n",
                " --> 1:1\n",
                "   |\n",
                " 1 | long_namespace/x\n",
                "   | ^^^^^^^^^^^^^^^^\n",
            )
        );
    }

    #[test]
    fn display_includes_named_runtime_stack_trace() {
        let mut vm = Vm::new();
        let error = vm
            .eval_str(
                vm.global_module(),
                "(define (inner) (+ #true)) (define (outer) (inner)) (outer)",
            )
            .expect_err("expected a runtime error");

        assert_eq!(
            error.display(&vm, VmErrorConfig::default()).to_string(),
            concat!(
                "error: expected number, found #true in <native-func +>\n",
                "\n",
                "backtrace:\n",
                "  0: <native-func +>\n",
                "  1: <func inner>\n",
                "  2: <func outer>\n",
                "  3: <anonymous func>\n",
            )
        );
    }

    #[test]
    fn display_includes_native_runtime_stack_trace() {
        let mut vm = Vm::new();
        let error = vm
            .eval_str(vm.global_module(), "(define (outer) (+ #true)) (outer)")
            .expect_err("expected a runtime error");

        assert_eq!(
            error.display(&vm, VmErrorConfig::default()).to_string(),
            concat!(
                "error: expected number, found #true in <native-func +>\n",
                "\n",
                "backtrace:\n",
                "  0: <native-func +>\n",
                "  1: <func outer>\n",
                "  2: <anonymous func>\n",
            )
        );
    }

    #[test]
    fn display_uses_correct_arity_grammar() {
        let mut vm = Vm::new();

        for (source, expected) in [
            (
                "(define (one value) value) (one)",
                concat!(
                    "error: expected 1 argument, found 0 arguments in <anonymous func>\n",
                    "\n",
                    "backtrace:\n",
                    "  0: <anonymous func>\n",
                ),
            ),
            (
                "(define (two first second) first) (two 1)",
                concat!(
                    "error: expected 2 arguments, found 1 argument in <anonymous func>\n",
                    "\n",
                    "backtrace:\n",
                    "  0: <anonymous func>\n",
                ),
            ),
        ] {
            let error = vm
                .eval_str(vm.global_module(), source)
                .expect_err("expected an arity error");
            assert_eq!(
                error.display(&vm, VmErrorConfig::default()).to_string(),
                expected
            );
        }
    }
}
