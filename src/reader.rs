use crate::{
    Val,
    tokenizer::{Span, Token, TokenKind, Tokenizer},
};

/// An error associated with [`ValReader`].
#[derive(Debug, PartialEq, Eq)]
pub struct ValReadError {
    pub kind: ValReadErrorKind,
    pub span: Span,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ValReadErrorKind {
    UnexpectedCloseParen,
    UnclosedParen,
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
}

impl Reader for ValReader {
    type Value = Val;
    type Err = ValReadError;

    fn start_list(&mut self, _: Token) -> Result<(), Self::Err> {
        self.lists.push(Vec::new());
        Ok(())
    }

    fn end_list(&mut self, token: Token) -> Result<(), Self::Err> {
        let val = self.lists.pop().map(Val::List).ok_or(ValReadError {
            kind: ValReadErrorKind::UnexpectedCloseParen,
            span: token.span,
        })?;
        match self.lists.last_mut() {
            Some(list) => list.push(val),
            None => self.atom = Some(val),
        };
        Ok(())
    }

    fn visit_atom(&mut self, atom: Atom, _: Token) -> Result<(), Self::Err> {
        let val = match atom {
            Atom::Identifier(ident) => Val::Symbol(ident.to_string()),
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

    fn unexpected_end_list(&mut self, token: Token) -> Self::Err {
        ValReadError {
            kind: ValReadErrorKind::UnexpectedCloseParen,
            span: token.span,
        }
    }

    fn unclosed_list(&mut self, token: Token) -> Self::Err {
        ValReadError {
            kind: ValReadErrorKind::UnclosedParen,
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

    /// Construct an unexpected end of list error. Occurs when a `)` token is encountered without a
    /// proper opening `(`.
    fn unexpected_end_list(&mut self, _: Token) -> Self::Err;

    /// Construct an unclosed list error. Occurs when an opening `(` is present but the closing `)` is never reached.
    fn unclosed_list(&mut self, _: Token) -> Self::Err;

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
                        return Err(self.0.unclosed_list(Token {
                            kind: TokenKind::OpenParen,
                            span,
                        }));
                    }
                    None => return Ok(None),
                };
            };
            match token.kind {
                TokenKind::OpenParen => {
                    self.0.start_list(token)?;
                    open_paren_spans.push(token.span);
                }
                TokenKind::CloseParen => {
                    if open_paren_spans.pop().is_none() {
                        return Err(self.0.unexpected_end_list(token));
                    }
                    self.0.end_list(token)?;
                    if open_paren_spans.is_empty() {
                        return Ok(self.0.finalize());
                    }
                }
                _ => {
                    let source = self.1.source(token.span);
                    let atom = Atom::Identifier(source);
                    self.0.visit_atom(atom, token)?;
                    if open_paren_spans.is_empty() {
                        return Ok(self.0.finalize());
                    }
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
                kind: ValReadErrorKind::UnclosedParen,
                span: (1..2).into()
            }))
        );
    }

    #[test]
    fn closed_paren_is_error() {
        assert_eq!(
            read_one(" )"),
            Some(Err(ValReadError {
                kind: ValReadErrorKind::UnexpectedCloseParen,
                span: (1..2).into()
            }))
        );
    }
}
