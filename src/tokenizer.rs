/// The type of for a token.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// An open parenthesis.
    OpenParen,
    /// A closing parenthesis.
    CloseParen,
    /// An identifier. Could be a keyword, symbol, or number.
    Identifier,
    /// A comment extending from `;` to the end of the line.
    Comment,
    /// Holds a string. Includes the outer quotes.
    String,
    /// A string that was not well formed. Usually its missing a closing quote.
    MalformedString,
}

/// Tokenizes a source string.
///
/// This turns a string into a stream of tokens. Tokens are delimited by [`TokenKind`] with a
/// [`Span`] that identifies where in the `source` string we are looking.
pub struct Tokenizer<'a> {
    source: &'a str,
    position: usize,
}

/// A simple range with [start, end). Meant to reference a substring within source.
pub type Span = std::ops::Range<usize>;

impl<'a> Tokenizer<'a> {
    /// Create a new Tokenizer.
    pub fn new(source: &'a str) -> Self {
        Tokenizer {
            source,
            position: 0,
        }
    }

    /// Return the underlying source for the given span.
    ///
    /// May panic if the `range` is out of range of `source`.
    pub fn source(&self, range: Span) -> &'a str {
        &self.source[range]
    }

    /// Get the next token kind and span. Returns `None` if we have explored all tokens.
    pub fn next_span(&mut self) -> Option<(TokenKind, Span)> {
        self.eat_whitespace();

        let start = self.position;
        let text = &self.source[self.position..];
        let leading_char = text.chars().next()?;
        let ret = match leading_char {
            '(' => {
                self.position += leading_char.len_utf8();
                (TokenKind::OpenParen, start..self.position)
            }
            ')' => {
                self.position += leading_char.len_utf8();
                (TokenKind::CloseParen, start..self.position)
            }
            ';' => (TokenKind::Comment, self.eat_comment()),
            '"' => self.eat_string(),
            _ => (TokenKind::Identifier, self.eat_identifier()),
        };
        Some(ret)
    }

    #[inline(always)]
    fn eat_while(&mut self, mut stop_pred: impl FnMut(char) -> bool) -> Span {
        let start = self.position;
        let text = &self.source[self.position..];
        for ch in text.chars() {
            if stop_pred(ch) {
                break;
            }
            self.position += ch.len_utf8();
        }
        start..self.position
    }

    fn eat_string(&mut self) -> (TokenKind, Span) {
        let start = self.position;
        // CALLER INVARIANT: The current string should be non-empty and start with a quote.
        self.position += '"'.len_utf8();
        let mut escaped = false;
        let inner = self.eat_while(|ch| {
            if escaped {
                escaped = false;
                return false;
            }
            match ch {
                '"' => true,
                '\\' => {
                    escaped = true;
                    false
                }
                _ => false,
            }
        });
        if escaped {
            return (TokenKind::MalformedString, start..self.position);
        }
        let is_closed = self.source[inner.end..].starts_with('"');
        if is_closed {
            self.position += '"'.len_utf8();
            (TokenKind::String, start..self.position)
        } else {
            (TokenKind::MalformedString, start..self.position)
        }
    }

    fn eat_identifier(&mut self) -> Span {
        self.eat_while(|ch| ch.is_whitespace() || ch == '(' || ch == ')')
    }

    fn eat_comment(&mut self) -> Span {
        self.eat_while(|ch| ch == '\n' || ch == '\r')
    }

    fn eat_whitespace(&mut self) -> Span {
        self.eat_while(|ch| !ch.is_whitespace())
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = (TokenKind, &'a str);

    /// Get the next token kind along with the text associated with it.
    fn next(&mut self) -> Option<(TokenKind, &'a str)> {
        let (kind, rng) = self.next_span()?;
        Some((kind, &self.source[rng]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_separated_by_whitespace_or_parens() {
        assert_eq!(
            Tokenizer::new("foo (bar) (baz one) two").collect::<Vec<_>>(),
            &[
                (TokenKind::Identifier, "foo"),
                (TokenKind::OpenParen, "("),
                (TokenKind::Identifier, "bar"),
                (TokenKind::CloseParen, ")"),
                (TokenKind::OpenParen, "("),
                (TokenKind::Identifier, "baz"),
                (TokenKind::Identifier, "one"),
                (TokenKind::CloseParen, ")"),
                (TokenKind::Identifier, "two"),
            ],
        )
    }

    #[test]
    fn string_is_parsed() {
        assert_eq!(
            Tokenizer::new("\"Hello()\" \" World\"").collect::<Vec<_>>(),
            &[
                (TokenKind::String, "\"Hello()\""),
                (TokenKind::String, "\" World\""),
            ],
        )
    }

    #[test]
    fn string_with_escaped_quotes_includes_quotes() {
        assert_eq!(
            Tokenizer::new("\"\\\"Hello World\\\"\"").collect::<Vec<_>>(),
            &[(TokenKind::String, "\"\\\"Hello World\\\"\"")],
        )
    }

    #[test]
    fn unterminated_string_is_parsed_as_malformed() {
        assert_eq!(
            Tokenizer::new("\"Hello()\" \" World").collect::<Vec<_>>(),
            &[
                (TokenKind::String, "\"Hello()\""),
                (TokenKind::MalformedString, "\" World"),
            ],
        )
    }

    #[test]
    fn unterminated_escape_is_malformed() {
        assert_eq!(
            Tokenizer::new("\"bad\\\"").collect::<Vec<_>>(),
            &[(TokenKind::MalformedString, "\"bad\\\"")],
        )
    }

    #[test]
    fn comments_are_parsed_until_the_end_of_the_line() {
        assert_eq!(
            Tokenizer::new("foo ; my comment\nbar").collect::<Vec<_>>(),
            &[
                (TokenKind::Identifier, "foo"),
                (TokenKind::Comment, "; my comment"),
                (TokenKind::Identifier, "bar"),
            ],
        )
    }

    #[test]
    fn multiline_comments_are_parsed_until_the_end_of_the_line() {
        assert_eq!(
            Tokenizer::new("foo ; my comment\n; continues here\nbar").collect::<Vec<_>>(),
            &[
                (TokenKind::Identifier, "foo"),
                (TokenKind::Comment, "; my comment"),
                (TokenKind::Comment, "; continues here"),
                (TokenKind::Identifier, "bar"),
            ],
        )
    }

    #[test]
    fn semicolons_inside_strings_are_not_comments() {
        assert_eq!(
            Tokenizer::new("\"a; b\"").collect::<Vec<_>>(),
            &[(TokenKind::String, "\"a; b\"")],
        )
    }
}
