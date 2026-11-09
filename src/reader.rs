use crate::{
    Val,
    tokenizer::{Span, Token, TokenKind, Tokenizer},
};

/// An error associated with [`ValReader`].
#[derive(Debug, PartialEq, Eq)]
pub struct ValReadError {
    pub kind: ReaderErrorKind,
    pub span: Span,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReaderErrorKind {
    UnexpectedCloseParen,
    UnclosedParen,
    UnterminatedString,
    InvalidEscape,
}

/// Parse text as [`Val`] objects.
#[derive(Default, Debug)]
pub struct ValReader {
    atom: Option<Val>,
    lists: Vec<Vec<Val>>,
}

/// A single element parsed from a token.
pub enum Atom<'a> {
    Identifier(&'a str),
    Int(i64),
    Float(f64),
    String(String),
}

impl<'a> Atom<'a> {
    /// Parse an identifier. Attempts to parse as an int or float and falls back to identifier for
    /// all others.
    pub fn parse_identifier(source: &'a str) -> Self {
        let unsigned = source.strip_prefix(['+', '-']).unwrap_or(source);
        let unsigned = unsigned.strip_prefix('.').unwrap_or(unsigned);
        if !unsigned.starts_with(|c: char| c.is_ascii_digit()) {
            return Self::Identifier(source);
        }
        if let Ok(i) = source.parse() {
            Self::Int(i)
        } else if let Ok(f) = source.parse() {
            Self::Float(f)
        } else {
            Self::Identifier(source)
        }
    }

    /// Parse a quoted string, decoding `\n`, `\t`, `\r`, `\0`, `\b`, `\f`,
    /// `\\`, and `\"` escapes. Unknown escapes are rejected.
    pub fn parse_string(source: &str) -> Result<Self, ReaderErrorKind> {
        let contents = source
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .ok_or(ReaderErrorKind::UnterminatedString)?;
        let mut chars = contents.chars();
        let mut value = String::with_capacity(contents.len());
        while let Some(ch) = chars.next() {
            let decoded_ch = if ch != '\\' {
                ch
            } else {
                match chars.next() {
                    Some('n') => '\n',
                    Some('t') => '\t',
                    Some('r') => '\r',
                    Some('0') => '\0',
                    Some('b') => '\u{0008}',
                    Some('f') => '\u{000c}',
                    Some('\\') => '\\',
                    Some('"') => '"',
                    Some(_) => return Err(ReaderErrorKind::InvalidEscape),
                    None => return Err(ReaderErrorKind::UnterminatedString),
                }
            };
            value.push(decoded_ch);
        }
        Ok(Self::String(value))
    }
}

impl Reader for ValReader {
    type Value = Val;
    type Err = ValReadError;

    fn start_list(&mut self, _: Token) -> Result<(), Self::Err> {
        self.lists.push(Vec::new());
        Ok(())
    }

    fn end_list(&mut self, token: Token) -> Result<(), Self::Err> {
        let val = self
            .lists
            .pop()
            .map(Val::List)
            .ok_or_else(|| self.error(ReaderErrorKind::UnexpectedCloseParen, token))?;
        match self.lists.last_mut() {
            Some(list) => list.push(val),
            None => self.atom = Some(val),
        };
        Ok(())
    }

    fn visit_atom(&mut self, atom: Atom, _: Token) -> Result<(), Self::Err> {
        let val = match atom {
            Atom::Identifier(ident) => Val::Symbol(ident.to_string()),
            Atom::Int(i) => Val::Int(i),
            Atom::Float(f) => Val::Float(f),
            Atom::String(s) => Val::String(s),
        };
        match self.lists.last_mut() {
            Some(list) => list.push(val),
            None => self.atom = Some(val),
        }
        Ok(())
    }

    fn finalize(&mut self) -> Option<Self::Value> {
        self.atom.take()
    }

    fn error(&mut self, kind: ReaderErrorKind, token: Token) -> Self::Err {
        ValReadError {
            kind,
            span: token.span,
        }
    }
}

pub trait Reader: Sized {
    /// The type to parse an expression into.
    type Value;

    /// The error type to use when reading fails.
    type Err;

    /// Called when the start of a list is encountered. Exits on error.
    fn start_list(&mut self, _: Token) -> Result<(), Self::Err>;

    /// Called when the end of alist is encountered. Exits on error.
    fn end_list(&mut self, _: Token) -> Result<(), Self::Err>;

    /// Called when an atom is encountered. Exits on error.
    fn visit_atom(&mut self, _: Atom, _: Token) -> Result<(), Self::Err>;

    /// Produce the final output. Returned when a just a single atom is encountered or when the
    /// entire list has been parsed.
    fn finalize(&mut self) -> Option<Self::Value>;

    /// Convert a reader error kind and its associated token into the desired error type.
    fn error(&mut self, kind: ReaderErrorKind, token: Token) -> Self::Err;

    //// Create an iterator that produces all values encountered for `source`.
    fn iter<'a>(self, source: &'a str) -> ReaderIter<'a, Self> {
        ReaderIter(self, Tokenizer::new(source))
    }
}

/// An iterator over values.
pub struct ReaderIter<'a, R>(R, Tokenizer<'a>);

impl<'a, R: Reader> Iterator for ReaderIter<'a, R> {
    type Item = Result<R::Value, R::Err>;

    fn next(&mut self) -> Option<Result<R::Value, R::Err>> {
        self.read_next().transpose()
    }
}

impl<'a, R: Reader> ReaderIter<'a, R> {
    fn read_next(&mut self) -> Result<Option<R::Value>, R::Err>
    where
        R: Reader,
    {
        let mut open_paren_spans: Vec<Span> = Vec::new();
        loop {
            let Some(token) = self.1.next() else {
                match open_paren_spans.pop() {
                    Some(span) => {
                        return Err(self.0.error(
                            ReaderErrorKind::UnclosedParen,
                            Token {
                                kind: TokenKind::OpenParen,
                                span,
                            },
                        ));
                    }
                    None => return Ok(None),
                };
            };
            let maybe_atom = match token.kind {
                TokenKind::OpenParen => {
                    self.0.start_list(token)?;
                    open_paren_spans.push(token.span);
                    None
                }
                TokenKind::CloseParen => {
                    if open_paren_spans.pop().is_none() {
                        return Err(self.0.error(ReaderErrorKind::UnexpectedCloseParen, token));
                    }
                    self.0.end_list(token)?;
                    if open_paren_spans.is_empty() {
                        return Ok(self.0.finalize());
                    }
                    None
                }
                TokenKind::String => {
                    let atom = Atom::parse_string(self.1.source(token.span))
                        .map_err(|kind| self.0.error(kind, token))?;
                    Some(atom)
                }
                TokenKind::UnterminatedString => {
                    return Err(self.0.error(ReaderErrorKind::UnterminatedString, token));
                }
                TokenKind::Identifier => {
                    let source = self.1.source(token.span);
                    Some(Atom::parse_identifier(source))
                }
            };
            if let Some(atom) = maybe_atom {
                self.0.visit_atom(atom, token)?;
                if open_paren_spans.is_empty() {
                    return Ok(self.0.finalize());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_one(source: &str) -> Option<Result<Val, ValReadError>> {
        let mut reader = ValReader::default().iter(source);
        let res = reader.next()?;
        assert!(reader.next().is_none());
        Some(res)
    }

    #[test]
    fn empty_returns_none() {
        assert_eq!(read_one(""), None);
    }

    #[test]
    fn symbol_returns_symbol() {
        assert_eq!(read_one("hello"), Some(Ok(Val::Symbol("hello".into()))));
    }

    #[test]
    fn numbers_return_int_or_float() {
        for (source, expected) in [
            ("42", Val::Int(42)),
            ("-7", Val::Int(-7)),
            ("+7", Val::Int(7)),
            ("1.5", Val::Float(1.5)),
            ("-0.25", Val::Float(-0.25)),
            (".5", Val::Float(0.5)),
            ("1e3", Val::Float(1000.0)),
        ] {
            assert_eq!(read_one(source), Some(Ok(expected)), "{source}");
        }
    }

    #[test]
    fn number_like_identifiers_return_symbols() {
        for source in ["-", "+", "...", "inf", "nan", "-inf", "1abc", "1.2.3"] {
            assert_eq!(
                read_one(source),
                Some(Ok(Val::Symbol(source.into()))),
                "{source}"
            );
        }
    }

    #[test]
    fn quoted_string_returns_string_value_without_quotes() {
        assert_eq!(
            read_one(r#""hello""#),
            Some(Ok(Val::String("hello".into())))
        );
    }

    #[test]
    fn empty_quoted_string_returns_empty_string() {
        assert_eq!(read_one(r#""""#), Some(Ok(Val::String(String::new()))));
    }

    #[test]
    fn valid_escapes_return_decoded_characters() {
        struct Case {
            source: &'static str,
            expected: &'static str,
        }

        for case in [
            Case {
                source: r#""a\nb""#,
                expected: "a\nb",
            },
            Case {
                source: r#""a\tb""#,
                expected: "a\tb",
            },
            Case {
                source: r#""a\rb""#,
                expected: "a\rb",
            },
            Case {
                source: r#""a\0b""#,
                expected: "a\0b",
            },
            Case {
                source: r#""a\bb""#,
                expected: "a\u{0008}b",
            },
            Case {
                source: r#""a\fb""#,
                expected: "a\u{000c}b",
            },
            Case {
                source: r#""say \"hello\" at C:\\tmp\\""#,
                expected: "say \"hello\" at C:\\tmp\\",
            },
            Case {
                source: r#""\\n""#,
                expected: r"\n",
            },
            Case {
                source: r#""héllo\n世界🐙""#,
                expected: "héllo\n世界🐙",
            },
        ] {
            assert_eq!(
                read_one(case.source),
                Some(Ok(Val::String(case.expected.into()))),
                "{}",
                case.source
            );
        }
        assert_eq!(
            read_one(r#"("a\nb" ("\t"))"#),
            Some(Ok(Val::List(vec![
                Val::String("a\nb".into()),
                Val::List(vec![Val::String("\t".into())]),
            ])))
        );
    }

    #[test]
    fn unknown_escape_reports_error_with_string_span() {
        let source = r#"  "bad\q""#;
        assert_eq!(
            read_one(source),
            Some(Err(ValReadError {
                kind: ReaderErrorKind::InvalidEscape,
                span: (2..source.len()).into(),
            }))
        );
    }

    #[test]
    fn list_containing_strings_and_symbols_preserves_values_and_order() {
        assert_eq!(
            read_one(r#"(first "hello" ("" last) "world")"#),
            Some(Ok(Val::List(vec![
                Val::Symbol("first".into()),
                Val::String("hello".into()),
                Val::List(vec![Val::String(String::new()), Val::Symbol("last".into())]),
                Val::String("world".into()),
            ])))
        );
    }

    #[test]
    fn adjacent_quoted_strings_return_separate_values() {
        let values: Vec<_> = ValReader::default().iter(r#""first""second""#).collect();
        assert_eq!(
            values,
            vec![
                Ok(Val::String("first".into())),
                Ok(Val::String("second".into()))
            ]
        );
    }

    #[test]
    fn unterminated_string_reports_error_with_string_span() {
        let source = r#"  "héllo"#;
        assert_eq!(
            read_one(source),
            Some(Err(ValReadError {
                kind: ReaderErrorKind::UnterminatedString,
                span: (2..source.len()).into(),
            }))
        );
    }

    #[test]
    fn unterminated_string_inside_list_reports_string_error() {
        let source = r#"(a "unfinished"#;
        assert_eq!(
            read_one(source),
            Some(Err(ValReadError {
                kind: ReaderErrorKind::UnterminatedString,
                span: (3..source.len()).into(),
            }))
        );
    }

    #[test]
    fn escaped_final_quote_without_closing_quote_reports_unterminated_string() {
        let source = r#""unfinished\""#;
        assert_eq!(
            read_one(source),
            Some(Err(ValReadError {
                kind: ReaderErrorKind::UnterminatedString,
                span: (0..source.len()).into(),
            }))
        );
    }

    #[test]
    fn parens_parsed_as_lists() {
        assert_eq!(
            read_one("(a b (c))"),
            Some(Ok(Val::List(vec![
                Val::Symbol("a".into()),
                Val::Symbol("b".into()),
                Val::List(vec![Val::Symbol("c".into())])
            ])))
        );
    }

    #[test]
    fn unclosed_paren_is_error() {
        assert_eq!(
            read_one(" (1 2 3"),
            Some(Err(ValReadError {
                kind: ReaderErrorKind::UnclosedParen,
                span: (1..2).into()
            }))
        );
    }

    #[test]
    fn closed_paren_is_error() {
        assert_eq!(
            read_one(" )"),
            Some(Err(ValReadError {
                kind: ReaderErrorKind::UnexpectedCloseParen,
                span: (1..2).into()
            }))
        );
    }
}
