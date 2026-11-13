use crate::{
    reader::{Atom, Reader, ReaderErrorKind},
    tokenizer::{Span, Token},
    val::Val,
};

/// An abstract syntax tree that can be converted into executable bytecode.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ast {
    /// The span of the Ast within its source code.
    pub span: Span,
    /// The data for the Ast node.
    pub kind: AstKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AstKind {
    Const(Val),
    Eval { func: Box<Ast>, args: Vec<Ast> },
    Lookup(String),
    Begin(Vec<Ast>),
    If(IfAst),
}

#[derive(Clone, Debug, PartialEq)]
pub struct IfAst {
    pub pred: Box<Ast>,
    pub then_branch: Box<Ast>,
    pub else_branch: Option<Box<Ast>>,
}

impl Default for AstKind {
    fn default() -> AstKind {
        AstKind::Const(Val::Nil)
    }
}

impl Ast {
    pub fn parse(source: &str) -> Result<Ast, AstBuilderError> {
        let mut asts = Vec::new();
        for ast_res in AstBuilder::default().iter(source) {
            asts.push(ast_res?);
        }
        match asts.len() {
            0 => Ok(Ast {
                kind: AstKind::Const(Val::Nil),
                span: (0..0).into(),
            }),
            1 => Ok(std::mem::take(asts.first_mut().unwrap())),
            _ => {
                let span = (asts[0].span.start..asts.last().unwrap().span.end).into();
                Ok(Ast {
                    kind: AstKind::Begin(asts),
                    span,
                })
            }
        }
    }
}

impl TryFrom<&'_ SExpr> for Ast {
    type Error = AstBuilderError;

    fn try_from(expr: &SExpr) -> Result<Ast, AstBuilderError> {
        let kind = match &expr.kind {
            SExprKind::Ident(ident) => AstKind::Lookup(ident.clone()),
            SExprKind::Const(val) => AstKind::Const(val.clone()),
            SExprKind::List(items) => AstKind::from_list(items, expr.span)?,
        };
        Ok(Ast {
            span: expr.span,
            kind,
        })
    }
}

impl AstKind {
    fn from_list(items: &[SExpr], span: Span) -> Result<AstKind, AstBuilderError> {
        match items {
            [] => Err(AstBuilderError {
                kind: AstBuilderErrorKind::EmptyExpression,
                span,
            }),
            [first, rest @ ..] => {
                if let SExprKind::Ident(ident) = &first.kind
                    && ident == "if"
                {
                    return Self::parse_if(rest, span);
                }
                let func = Box::new(Ast::try_from(first)?);
                let args = rest
                    .iter()
                    .map(Ast::try_from)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(AstKind::Eval { func, args })
            }
        }
    }

    fn parse_if(args: &[SExpr], span: Span) -> Result<AstKind, AstBuilderError> {
        match args {
            [pred, then_branch] => Ok(AstKind::If(IfAst {
                pred: Box::new(Ast::try_from(pred)?),
                then_branch: Box::new(Ast::try_from(then_branch)?),
                else_branch: None,
            })),
            [pred, then_branch, else_branch] => Ok(AstKind::If(IfAst {
                pred: Box::new(Ast::try_from(pred)?),
                then_branch: Box::new(Ast::try_from(then_branch)?),
                else_branch: Some(Box::new(Ast::try_from(else_branch)?)),
            })),
            _ => Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidArgCount {
                    expected: "2 or 3",
                    actual: args.len(),
                },
                span,
            }),
        }
    }
}

struct SExpr {
    span: Span,
    kind: SExprKind,
}

enum SExprKind {
    Ident(String),
    Const(Val),
    List(Vec<SExpr>),
}

struct OpenList {
    start: usize,
    items: Vec<SExpr>,
}

#[derive(Default)]
pub struct AstBuilder {
    expr: Option<SExpr>,
    open_lists: Vec<OpenList>,
}

impl AstBuilder {
    fn push_expr(&mut self, expr: SExpr) {
        match self.open_lists.last_mut() {
            Some(list) => list.items.push(expr),
            None => self.expr = Some(expr),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AstBuilderError {
    kind: AstBuilderErrorKind,
    span: Span,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum AstBuilderErrorKind {
    UnexpectedCloseParen,
    UnclosedParen,
    UnterminatedString,
    InvalidEscape,
    EmptyExpression,
    InvalidArgCount {
        expected: &'static str,
        actual: usize,
    },
}

impl Reader for AstBuilder {
    type Value = Ast;

    type Err = AstBuilderError;

    fn reset(&mut self) {
        self.open_lists.clear();
        self.expr.take();
    }

    fn start_list(&mut self, token: Token) -> Result<(), AstBuilderError> {
        self.open_lists.push(OpenList {
            start: token.span.start,
            items: Vec::new(),
        });
        Ok(())
    }

    fn end_list(&mut self, token: Token) -> Result<(), AstBuilderError> {
        let open = self.open_lists.pop().ok_or(AstBuilderError {
            kind: AstBuilderErrorKind::UnexpectedCloseParen,
            span: token.span,
        })?;
        self.push_expr(SExpr {
            span: (open.start..token.span.end).into(),
            kind: SExprKind::List(open.items),
        });
        Ok(())
    }

    fn visit_atom(&mut self, atom: Atom, token: Token) -> Result<(), AstBuilderError> {
        let kind = match atom {
            Atom::Identifier(ident) => SExprKind::Ident(ident.to_string()),
            other => SExprKind::Const(other.to_val()),
        };
        self.push_expr(SExpr {
            kind,
            span: token.span,
        });
        Ok(())
    }

    fn finalize(&mut self) -> Result<Option<Ast>, AstBuilderError> {
        self.expr.as_ref().map(Ast::try_from).transpose()
    }

    fn error(&mut self, kind: ReaderErrorKind, token: Token) -> AstBuilderError {
        let kind = match kind {
            ReaderErrorKind::UnexpectedCloseParen => AstBuilderErrorKind::UnexpectedCloseParen,
            ReaderErrorKind::UnclosedParen => AstBuilderErrorKind::UnclosedParen,
            ReaderErrorKind::UnterminatedString => AstBuilderErrorKind::UnterminatedString,
            ReaderErrorKind::InvalidEscape => AstBuilderErrorKind::InvalidEscape,
        };
        AstBuilderError {
            kind,
            span: token.span,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_source_is_empty() {
        assert_eq!(
            Ast::parse("\n"),
            Ok(Ast {
                kind: AstKind::Const(Val::Nil),
                span: (0..0).into(),
            })
        );
    }

    #[test]
    fn literal_is_constant() {
        assert_eq!(
            Ast::parse("1 2.0 \"hello\""),
            Ok(Ast {
                kind: AstKind::Begin(vec![
                    Ast {
                        kind: AstKind::Const(Val::Int(1)),
                        span: (0..1).into(),
                    },
                    Ast {
                        kind: AstKind::Const(Val::Float(2.0)),
                        span: (2..5).into()
                    },
                    Ast {
                        kind: AstKind::Const(Val::String("hello".into())),
                        span: (6..13).into()
                    }
                ]),
                span: (0..13).into(),
            })
        );
    }

    #[test]
    fn identifier_is_lookup() {
        assert_eq!(
            Ast::parse("identifier"),
            Ok(Ast {
                kind: AstKind::Lookup("identifier".into()),
                span: (0..10).into()
            })
        );
    }

    #[test]
    fn nested_expr_is_parsed() {
        assert_eq!(
            Ast::parse("(a (b c))"),
            Ok(Ast {
                kind: AstKind::Eval {
                    func: Box::new(Ast {
                        kind: AstKind::Lookup("a".into()),
                        span: (1..2).into()
                    }),
                    args: vec![Ast {
                        kind: AstKind::Eval {
                            func: Box::new(Ast {
                                kind: AstKind::Lookup("b".into()),
                                span: (4..5).into()
                            }),
                            args: vec![Ast {
                                kind: AstKind::Lookup("c".into()),
                                span: (6..7).into(),
                            }],
                        },
                        span: (3..8).into(),
                    },],
                },
                span: (0..9).into(),
            })
        );
    }

    #[test]
    fn empty_expr_is_error() {
        assert_eq!(
            Ast::parse("()"),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::EmptyExpression,
                span: (0..2).into()
            })
        );
    }

    #[test]
    fn if_parsed_as_if_expr() {
        assert_eq!(
            Ast::parse("(if #true 1)"),
            Ok(Ast {
                kind: AstKind::If(IfAst {
                    pred: Box::new(Ast {
                        kind: AstKind::Const(Val::Bool(true)),
                        span: (4..9).into()
                    }),
                    then_branch: Box::new(Ast {
                        kind: AstKind::Const(Val::Int(1)),
                        span: (10..11).into()
                    }),
                    else_branch: None,
                }),
                span: (0..12).into()
            })
        );
        assert_eq!(
            Ast::parse("(if #true 1 2)"),
            Ok(Ast {
                kind: AstKind::If(IfAst {
                    pred: Box::new(Ast {
                        kind: AstKind::Const(Val::Bool(true)),
                        span: (4..9).into()
                    }),
                    then_branch: Box::new(Ast {
                        kind: AstKind::Const(Val::Int(1)),
                        span: (10..11).into()
                    }),
                    else_branch: Some(Box::new(Ast {
                        kind: AstKind::Const(Val::Int(2)),
                        span: (12..13).into()
                    })),
                }),
                span: (0..14).into()
            })
        );
    }

    #[test]
    fn if_wrong_arg_count_is_error() {
        for (source, actual) in [("(if)", 0), ("(if #true)", 1), ("(if #true 1 2 3)", 4)] {
            assert_eq!(
                Ast::parse(source),
                Err(AstBuilderError {
                    kind: AstBuilderErrorKind::InvalidArgCount {
                        expected: "2 or 3",
                        actual,
                    },
                    span: (0..source.len()).into(),
                }),
                "{source}"
            );
        }
    }
}
