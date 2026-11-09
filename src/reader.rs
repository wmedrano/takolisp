use crate::{
    Val,
    tokenizer::{Span, Tokenizer},
};

/// An error associated with [`ValReader`].
#[derive(Debug, PartialEq, Eq)]
pub enum ValReadError {
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

    fn start_list(&mut self, _: Span) -> Result<(), Self::Err> {
        self.lists.push(Vec::new());
        Ok(())
    }

    fn end_list(&mut self, _: Span) -> Result<(), Self::Err> {
        let val = self
            .lists
            .pop()
            .map(Val::List)
            .ok_or(ValReadError::UnexpectedCloseParen)?;
        match self.lists.last_mut() {
            Some(list) => list.push(val),
            None => self.atom = Some(val),
        };
        Ok(())
    }

    fn visit_atom(&mut self, atom: Atom, _: Span) -> Result<(), Self::Err> {
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

    fn unexpected_end_list(&mut self, _: Span) -> Self::Err {
        ValReadError::UnexpectedCloseParen
    }

    fn unclosed_list(&mut self) -> Self::Err {
        ValReadError::UnclosedParen
    }
}

pub trait Reader: Sized {
    /// The type to parse an expression into.
    type Value;

    /// The error type to use when reading fails.
    type Err;

    /// Called when the start of a list is encountered. Exits on error.
    fn start_list(&mut self, _: Span) -> Result<(), Self::Err>;

    /// Called when the end of alist is encountered. Exits on error.
    fn end_list(&mut self, _: Span) -> Result<(), Self::Err>;

    /// Called when an atom is encountered. Exits on error.
    fn visit_atom(&mut self, _: Atom, _: Span) -> Result<(), Self::Err>;

    /// Produce the final output. Returned when a just a single atom is encountered or when the
    /// entire list has been parsed.
    fn finalize(&mut self) -> Option<Self::Value>;

    /// Construct an unexpected end of list error. Occurs when a `)` token is encountered without a
    /// proper opening `(`.
    fn unexpected_end_list(&mut self, _: Span) -> Self::Err;

    /// Construct an unclosed list error. Occurs when an opening `(` is present but the closing `)` is never reached.
    fn unclosed_list(&mut self) -> Self::Err;

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
        let mut nesting_depth: usize = 0;
        loop {
            let Some(token) = self.1.next() else {
                if nesting_depth == 0 {
                    return Ok(None);
                } else {
                    return Err(self.0.unclosed_list());
                }
            };
            let source = self.1.source(token.span);
            match source {
                "(" => {
                    self.0.start_list(token.span)?;
                    nesting_depth += 1;
                }
                ")" => {
                    nesting_depth = match nesting_depth.checked_sub(1) {
                        Some(x) => x,
                        None => return Err(self.0.unexpected_end_list(token.span)),
                    };
                    self.0.end_list(token.span)?;
                    if nesting_depth == 0 {
                        return Ok(self.0.finalize());
                    }
                }
                _ => {
                    let atom = Atom::Identifier(source);
                    self.0.visit_atom(atom, token.span)?;
                    if nesting_depth == 0 {
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
        assert_eq!(read_one("(1 2 3"), Some(Err(ValReadError::UnclosedParen)));
    }

    #[test]
    fn closed_paren_is_error() {
        assert_eq!(read_one(")"), Some(Err(ValReadError::UnexpectedCloseParen)));
    }
}
