use crate::{
    ModuleId,
    reader::{Atom, Reader, ReaderErrorKind},
    tokenizer::{Span, Token},
    val::{Symbol, Val},
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
    Eval {
        func: Box<Ast>,
        args: Vec<Ast>,
    },
    Define {
        name: Symbol,
        value: Box<Ast>,
    },
    Lookup {
        module: ModuleId,
        name: Symbol,
    },
    Begin(Vec<Ast>),
    TopLevel(Vec<Ast>),
    If {
        pred: Box<Ast>,
        then_branch: Box<Ast>,
        else_branch: Option<Box<Ast>>,
    },
    Lambda {
        name: Option<Symbol>,
        parameters: Vec<Symbol>,
        body: Vec<Ast>,
    },
    Import {
        module: ModuleId,
        alias: Symbol,
    },
}

impl Default for AstKind {
    fn default() -> AstKind {
        AstKind::Const(Val::Nil)
    }
}

impl Ast {
    pub fn parse_in(module: ModuleId, source: &str) -> Result<Ast, AstBuilderError> {
        let mut asts = Vec::new();
        for ast_res in AstBuilder::new(module).iter(source) {
            asts.push(ast_res?);
        }
        match asts.as_mut_slice() {
            [] => Ok(Ast {
                kind: AstKind::Const(Val::Nil),
                span: (0..0).into(),
            }),
            [ast] => Ok(std::mem::take(ast)),
            [first, .., last] => {
                let span = (first.span.start..last.span.end).into();
                Ok(Ast {
                    kind: AstKind::TopLevel(asts),
                    span,
                })
            }
        }
    }

    fn from_sexpr(expr: &SExpr, module: &ModuleId) -> Result<Ast, AstBuilderError> {
        let kind = match &expr.kind {
            SExprKind::Ident(ident) => AstKind::Lookup {
                module: module.clone(),
                name: Symbol::from(ident.as_str()),
            },
            SExprKind::Const(val) => AstKind::Const(val.clone()),
            SExprKind::List(items) => AstKind::from_list(items, expr.span, module)?,
        };
        Ok(Ast {
            span: expr.span,
            kind,
        })
    }
}

impl AstKind {
    fn from_list(
        items: &[SExpr],
        span: Span,
        module: &ModuleId,
    ) -> Result<AstKind, AstBuilderError> {
        match items {
            [] => Err(AstBuilderError {
                kind: AstBuilderErrorKind::EmptyExpression,
                span,
            }),
            [first, rest @ ..] => {
                if let SExprKind::Ident(ident) = &first.kind {
                    match ident.as_str() {
                        "define" => return Self::parse_define(rest, span, module),
                        "if" => return Self::parse_if(rest, span, module),
                        "lambda" => return Self::parse_lambda(rest, span, module),
                        "import" => return Self::parse_import(rest, span, module),
                        _ => {}
                    }
                }
                let func = Box::new(Ast::from_sexpr(first, module)?);
                let args = rest
                    .iter()
                    .map(|expr| Ast::from_sexpr(expr, module))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(AstKind::Eval { func, args })
            }
        }
    }

    fn parse_define(
        args: &[SExpr],
        span: Span,
        module: &ModuleId,
    ) -> Result<AstKind, AstBuilderError> {
        let [target, rest @ ..] = args else {
            return Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidArgCount {
                    expected: "2",
                    actual: 0,
                },
                span,
            });
        };
        match &target.kind {
            SExprKind::Ident(ident) => {
                let [value] = rest else {
                    return Err(AstBuilderError {
                        kind: AstBuilderErrorKind::InvalidArgCount {
                            expected: "2",
                            actual: args.len(),
                        },
                        span,
                    });
                };
                Ok(AstKind::Define {
                    name: Symbol::from(ident.as_str()),
                    value: Box::new(Ast::from_sexpr(value, module)?),
                })
            }
            SExprKind::List(sig) => {
                let [fn_name, params @ ..] = sig.as_slice() else {
                    return Err(AstBuilderError {
                        kind: AstBuilderErrorKind::InvalidDefineTarget,
                        span: target.span,
                    });
                };
                let SExprKind::Ident(ident) = &fn_name.kind else {
                    return Err(AstBuilderError {
                        kind: AstBuilderErrorKind::InvalidDefineTarget,
                        span: fn_name.span,
                    });
                };
                let lambda_end = rest.last().map_or(target.span.end, |last| last.span.end);
                let lambda_span = (target.span.start..lambda_end).into();
                let name = Symbol::from(ident.as_str());
                Ok(AstKind::Define {
                    name: name.clone(),
                    value: Box::new(Ast {
                        span: lambda_span,
                        kind: Self::build_lambda(Some(name), params, rest, module)?,
                    }),
                })
            }
            SExprKind::Const(_) => Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidDefineTarget,
                span: target.span,
            }),
        }
    }

    fn parse_if(args: &[SExpr], span: Span, module: &ModuleId) -> Result<AstKind, AstBuilderError> {
        match args {
            [pred, then_branch] => Ok(AstKind::If {
                pred: Box::new(Ast::from_sexpr(pred, module)?),
                then_branch: Box::new(Ast::from_sexpr(then_branch, module)?),
                else_branch: None,
            }),
            [pred, then_branch, else_branch] => Ok(AstKind::If {
                pred: Box::new(Ast::from_sexpr(pred, module)?),
                then_branch: Box::new(Ast::from_sexpr(then_branch, module)?),
                else_branch: Some(Box::new(Ast::from_sexpr(else_branch, module)?)),
            }),
            _ => Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidArgCount {
                    expected: "2 or 3",
                    actual: args.len(),
                },
                span,
            }),
        }
    }

    fn parse_lambda(
        args: &[SExpr],
        span: Span,
        module: &ModuleId,
    ) -> Result<AstKind, AstBuilderError> {
        let [params, body @ ..] = args else {
            return Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidArgCount {
                    expected: "list params with body",
                    actual: args.len(),
                },
                span,
            });
        };
        let SExprKind::List(params) = &params.kind else {
            return Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidLambdaParameters,
                span,
            });
        };
        Self::build_lambda(None, params, body, module)
    }

    fn build_lambda(
        name: Option<Symbol>,
        params: &[SExpr],
        body: &[SExpr],
        module: &ModuleId,
    ) -> Result<AstKind, AstBuilderError> {
        let mut parameters = Vec::with_capacity(params.len());
        for param in params {
            let SExprKind::Ident(name) = &param.kind else {
                return Err(AstBuilderError {
                    kind: AstBuilderErrorKind::InvalidLambdaParameters,
                    span: param.span,
                });
            };
            parameters.push(Symbol::from(name.as_str()));
        }
        let body = body
            .iter()
            .map(|expr| Ast::from_sexpr(expr, module))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(AstKind::Lambda {
            name,
            parameters,
            body,
        })
    }

    fn parse_import(
        args: &[SExpr],
        span: Span,
        module: &ModuleId,
    ) -> Result<AstKind, AstBuilderError> {
        let (path_expr, alias_expr) = match args {
            [path] => (path, None),
            [path, as_kw, alias] => {
                if !matches!(&as_kw.kind, SExprKind::Ident(kw) if kw == "as") {
                    return Err(AstBuilderError {
                        kind: AstBuilderErrorKind::InvalidImport,
                        span: as_kw.span,
                    });
                }
                (path, Some(alias))
            }
            _ => {
                return Err(AstBuilderError {
                    kind: AstBuilderErrorKind::InvalidArgCount {
                        expected: "1 or 3",
                        actual: args.len(),
                    },
                    span,
                });
            }
        };

        let SExprKind::Const(Val::String(path)) = &path_expr.kind else {
            return Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidImport,
                span: path_expr.span,
            });
        };

        let alias = match alias_expr {
            Some(expr) => {
                let SExprKind::Ident(ident) = &expr.kind else {
                    return Err(AstBuilderError {
                        kind: AstBuilderErrorKind::InvalidImport,
                        span: expr.span,
                    });
                };
                Symbol::from(ident.as_str())
            }
            None => {
                let Some(stem) = std::path::Path::new(path.as_str())
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .filter(|s| !s.is_empty())
                else {
                    return Err(AstBuilderError {
                        kind: AstBuilderErrorKind::InvalidImport,
                        span: path_expr.span,
                    });
                };
                Symbol::from(stem)
            }
        };

        let import_path = std::path::Path::new(path.as_str());
        let resolved_module = match module {
            ModuleId::File(current_file) if import_path.is_relative() => {
                match current_file.parent() {
                    Some(parent) => ModuleId::file(parent.join(import_path)),
                    None => ModuleId::file(import_path),
                }
            }
            _ => ModuleId::file(import_path),
        };

        Ok(AstKind::Import {
            module: resolved_module,
            alias,
        })
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

pub struct AstBuilder {
    module: ModuleId,
    expr: Option<SExpr>,
    open_lists: Vec<OpenList>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AstBuilderError {
    kind: AstBuilderErrorKind,
    span: Span,
}

#[non_exhaustive]
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
    InvalidLambdaParameters,
    InvalidDefineTarget,
    InvalidImport,
    NotImplemented(&'static str),
}

impl Default for AstBuilder {
    fn default() -> Self {
        Self::new(ModuleId::USER)
    }
}

impl AstBuilder {
    pub fn new(module: ModuleId) -> Self {
        Self {
            module,
            expr: None,
            open_lists: Vec::new(),
        }
    }

    fn push_expr(&mut self, expr: SExpr) {
        match self.open_lists.last_mut() {
            Some(list) => list.items.push(expr),
            None => self.expr = Some(expr),
        }
    }
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
        self.expr
            .as_ref()
            .map(|expr| Ast::from_sexpr(expr, &self.module))
            .transpose()
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
            Ast::parse_in(ModuleId::USER, "\n"),
            Ok(Ast {
                kind: AstKind::Const(Val::Nil),
                span: (0..0).into(),
            })
        );
    }

    #[test]
    fn literal_is_constant() {
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "1 2.0 \"hello\""),
            Ok(Ast {
                kind: AstKind::TopLevel(vec![
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
            Ast::parse_in(ModuleId::USER, "identifier"),
            Ok(Ast {
                kind: AstKind::Lookup {
                    module: ModuleId::USER,
                    name: "identifier".into(),
                },
                span: (0..10).into()
            })
        );
    }

    #[test]
    fn parse_in_records_module_on_lookups() {
        let module = ModuleId::internal("math");
        assert_eq!(
            Ast::parse_in(module.clone(), "(+ x 1)"),
            Ok(Ast {
                kind: AstKind::Eval {
                    func: Box::new(Ast {
                        kind: AstKind::Lookup {
                            module: module.clone(),
                            name: "+".into(),
                        },
                        span: (1..2).into(),
                    }),
                    args: vec![
                        Ast {
                            kind: AstKind::Lookup {
                                module,
                                name: "x".into(),
                            },
                            span: (3..4).into(),
                        },
                        Ast {
                            kind: AstKind::Const(Val::Int(1)),
                            span: (5..6).into(),
                        },
                    ],
                },
                span: (0..7).into(),
            })
        );
    }

    #[test]
    fn nested_expr_is_parsed() {
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(a (b c))"),
            Ok(Ast {
                kind: AstKind::Eval {
                    func: Box::new(Ast {
                        kind: AstKind::Lookup {
                            module: ModuleId::USER,
                            name: "a".into(),
                        },
                        span: (1..2).into()
                    }),
                    args: vec![Ast {
                        kind: AstKind::Eval {
                            func: Box::new(Ast {
                                kind: AstKind::Lookup {
                                    module: ModuleId::USER,
                                    name: "b".into(),
                                },
                                span: (4..5).into()
                            }),
                            args: vec![Ast {
                                kind: AstKind::Lookup {
                                    module: ModuleId::USER,
                                    name: "c".into(),
                                },
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
            Ast::parse_in(ModuleId::USER, "()"),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::EmptyExpression,
                span: (0..2).into()
            })
        );
    }

    #[test]
    fn if_parsed_as_if_expr() {
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(if #true 1)"),
            Ok(Ast {
                kind: AstKind::If {
                    pred: Box::new(Ast {
                        kind: AstKind::Const(Val::Bool(true)),
                        span: (4..9).into()
                    }),
                    then_branch: Box::new(Ast {
                        kind: AstKind::Const(Val::Int(1)),
                        span: (10..11).into()
                    }),
                    else_branch: None,
                },
                span: (0..12).into()
            })
        );
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(if #true 1 2)"),
            Ok(Ast {
                kind: AstKind::If {
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
                },
                span: (0..14).into()
            })
        );
    }

    #[test]
    fn if_wrong_arg_count_is_error() {
        for (source, actual) in [("(if)", 0), ("(if #true)", 1), ("(if #true 1 2 3)", 4)] {
            assert_eq!(
                Ast::parse_in(ModuleId::USER, source),
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

    #[test]
    fn lambda_with_non_symbol_param_is_error() {
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(lambda (1) 1)"),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidLambdaParameters,
                span: (9..10).into(),
            })
        );
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(lambda (x (y)) x)"),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidLambdaParameters,
                span: (11..14).into(),
            })
        );
    }

    #[test]
    fn lambda_returns_lambda_node() {
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(lambda ())"),
            Ok(Ast {
                kind: AstKind::Lambda {
                    name: None,
                    parameters: vec![],
                    body: vec![]
                },
                span: (0..11).into(),
            })
        );
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(lambda () 1 2)"),
            Ok(Ast {
                kind: AstKind::Lambda {
                    name: None,
                    parameters: vec![],
                    body: vec![
                        Ast {
                            kind: AstKind::Const(Val::Int(1)),
                            span: (11..12).into(),
                        },
                        Ast {
                            kind: AstKind::Const(Val::Int(2)),
                            span: (13..14).into(),
                        },
                    ],
                },
                span: (0..15).into(),
            })
        );
    }

    #[test]
    fn lambda_bad_params_is_error() {
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(lambda)"),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidArgCount {
                    expected: "list params with body",
                    actual: 0,
                },
                span: (0..8).into(),
            })
        );
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(lambda 1 2)"),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidLambdaParameters,
                span: (0..12).into(),
            }),
        );
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(lambda x x)"),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidLambdaParameters,
                span: (0..12).into(),
            }),
        );
    }

    #[test]
    fn define_parsed_as_define_node() {
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(define x 1)"),
            Ok(Ast {
                kind: AstKind::Define {
                    name: "x".into(),
                    value: Box::new(Ast {
                        kind: AstKind::Const(Val::Int(1)),
                        span: (10..11).into(),
                    }),
                },
                span: (0..12).into(),
            })
        );
    }

    #[test]
    fn define_wrong_arg_count_is_error() {
        for (source, actual) in [("(define)", 0), ("(define x)", 1), ("(define x 1 2)", 3)] {
            assert_eq!(
                Ast::parse_in(ModuleId::USER, source),
                Err(AstBuilderError {
                    kind: AstBuilderErrorKind::InvalidArgCount {
                        expected: "2",
                        actual,
                    },
                    span: (0..source.len()).into(),
                }),
                "{source}"
            );
        }
    }

    #[test]
    fn define_non_symbol_target_is_error() {
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(define 1 2)"),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidDefineTarget,
                span: (8..9).into(),
            })
        );
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(define () 1)"),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidDefineTarget,
                span: (8..10).into(),
            })
        );
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(define (1 x) x)"),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidDefineTarget,
                span: (9..10).into(),
            })
        );
    }

    #[test]
    fn define_lambda_parsed_as_define_with_lambda() {
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(define (foo bar) (+ bar 1))"),
            Ok(Ast {
                kind: AstKind::Define {
                    name: "foo".into(),
                    value: Box::new(Ast {
                        kind: AstKind::Lambda {
                            name: Some("foo".into()),
                            parameters: vec!["bar".into()],
                            body: vec![Ast {
                                kind: AstKind::Eval {
                                    func: Box::new(Ast {
                                        kind: AstKind::Lookup {
                                            module: ModuleId::USER,
                                            name: "+".into(),
                                        },
                                        span: (19..20).into(),
                                    }),
                                    args: vec![
                                        Ast {
                                            kind: AstKind::Lookup {
                                                module: ModuleId::USER,
                                                name: "bar".into(),
                                            },
                                            span: (21..24).into(),
                                        },
                                        Ast {
                                            kind: AstKind::Const(Val::Int(1)),
                                            span: (25..26).into(),
                                        },
                                    ],
                                },
                                span: (18..27).into(),
                            }],
                        },
                        span: (8..27).into(),
                    }),
                },
                span: (0..28).into(),
            })
        );
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(define (foo) 1 2)"),
            Ok(Ast {
                kind: AstKind::Define {
                    name: "foo".into(),
                    value: Box::new(Ast {
                        kind: AstKind::Lambda {
                            name: Some("foo".into()),
                            parameters: vec![],
                            body: vec![
                                Ast {
                                    kind: AstKind::Const(Val::Int(1)),
                                    span: (14..15).into(),
                                },
                                Ast {
                                    kind: AstKind::Const(Val::Int(2)),
                                    span: (16..17).into(),
                                },
                            ],
                        },
                        span: (8..17).into(),
                    }),
                },
                span: (0..18).into(),
            })
        );
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(define (foo))"),
            Ok(Ast {
                kind: AstKind::Define {
                    name: "foo".into(),
                    value: Box::new(Ast {
                        kind: AstKind::Lambda {
                            name: Some("foo".into()),
                            parameters: vec![],
                            body: vec![],
                        },
                        span: (8..13).into(),
                    }),
                },
                span: (0..14).into(),
            })
        );
    }

    #[test]
    fn define_lambda_with_non_symbol_param_is_error() {
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(define (foo 1) 1)"),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidLambdaParameters,
                span: (13..14).into(),
            })
        );
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(define (foo x (y)) x)"),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidLambdaParameters,
                span: (15..18).into(),
            })
        );
    }

    #[test]
    fn import_parses_path_with_default_or_explicit_alias() {
        assert_eq!(
            Ast::parse_in(ModuleId::USER, r#"(import "path/to/file.tl")"#),
            Ok(Ast {
                kind: AstKind::Import {
                    module: ModuleId::file("path/to/file.tl"),
                    alias: "file".into(),
                },
                span: (0..26).into(),
            })
        );
        assert_eq!(
            Ast::parse_in(ModuleId::USER, r#"(import "path/to/file.tl" as f)"#),
            Ok(Ast {
                kind: AstKind::Import {
                    module: ModuleId::file("path/to/file.tl"),
                    alias: "f".into(),
                },
                span: (0..31).into(),
            })
        );
    }

    #[test]
    fn import_resolves_relative_and_normalized_paths() {
        assert_eq!(
            Ast::parse_in(
                ModuleId::file("dir/sub/main.tl"),
                r#"(import "../shared/./math.tl")"#,
            ),
            Ok(Ast {
                kind: AstKind::Import {
                    module: ModuleId::file("dir/shared/math.tl"),
                    alias: "math".into(),
                },
                span: (0..30).into(),
            })
        );
        assert_eq!(
            Ast::parse_in(
                ModuleId::file("dir/sub/main.tl"),
                r#"(import "/abs/../lib/math.tl" as m)"#,
            ),
            Ok(Ast {
                kind: AstKind::Import {
                    module: ModuleId::file("/lib/math.tl"),
                    alias: "m".into(),
                },
                span: (0..35).into(),
            })
        );
    }

    #[test]
    fn import_invalid_forms_return_error() {
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(import)"),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidArgCount {
                    expected: "1 or 3",
                    actual: 0,
                },
                span: (0..8).into(),
            })
        );
        assert_eq!(
            Ast::parse_in(ModuleId::USER, r#"(import "path/to/file.tl" f)"#),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidArgCount {
                    expected: "1 or 3",
                    actual: 2,
                },
                span: (0..28).into(),
            })
        );
        assert_eq!(
            Ast::parse_in(ModuleId::USER, "(import file)"),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidImport,
                span: (8..12).into(),
            })
        );
        assert_eq!(
            Ast::parse_in(ModuleId::USER, r#"(import "path/to/file.tl" to f)"#),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidImport,
                span: (26..28).into(),
            })
        );
        assert_eq!(
            Ast::parse_in(ModuleId::USER, r#"(import "path/to/file.tl" as 1)"#),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidImport,
                span: (29..30).into(),
            })
        );
        assert_eq!(
            Ast::parse_in(ModuleId::USER, r#"(import "")"#),
            Err(AstBuilderError {
                kind: AstBuilderErrorKind::InvalidImport,
                span: (8..10).into(),
            })
        );
    }
}
