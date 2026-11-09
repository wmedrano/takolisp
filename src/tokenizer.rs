use core::range::Range;

pub type Span = Range<usize>;

/// Refers to a subset of a source string.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Token {
    /// The location within the source.
    pub span: Span,
    /// The kind of token.
    pub kind: TokenKind,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TokenKind {
    OpenParen,
    CloseParen,
    Identifier,
    String,
    UnterminatedString,
}

/// Converts text into a stream of tokens.
pub struct Tokenizer<'a> {
    position: usize,
    source: &'a str,
}

impl<'a> Tokenizer<'a> {
    /// Create a new [`Tokenizer`] from a source string.
    pub fn new(source: &'a str) -> Self {
        Tokenizer {
            position: 0,
            source,
        }
    }

    /// Get the source string for the corresponding [`Span`].
    pub fn source(&self, span: Span) -> &str {
        &self.source[span]
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Token;

    fn next(&mut self) -> Option<Token> {
        self.eat_whitespace();
        let start = self.position;
        let kind = self.eat_next()?;
        let end = self.position;
        let span = (start != end).then(|| (start..end).into())?;
        Some(Token { span, kind })
    }
}

impl<'a> Tokenizer<'a> {
    fn eat_until(&mut self, mut pred: impl FnMut(char) -> bool) {
        let source = &self.source[self.position..];
        for ch in source.chars() {
            if pred(ch) {
                return;
            }
            self.position += ch.len_utf8();
        }
    }

    fn eat_whitespace(&mut self) {
        self.eat_until(|ch| !ch.is_whitespace());
    }

    fn eat_next(&mut self) -> Option<TokenKind> {
        let Some(leading_ch) = self.source[self.position..].chars().next() else {
            return None;
        };
        match leading_ch {
            '"' => Some(self.eat_string()),
            '(' => {
                self.position += leading_ch.len_utf8();
                Some(TokenKind::OpenParen)
            }
            ')' => {
                self.position += leading_ch.len_utf8();
                Some(TokenKind::CloseParen)
            }
            _ => {
                self.eat_until(|ch| ch.is_whitespace() || matches!(ch, '(' | ')' | '"'));
                Some(TokenKind::Identifier)
            }
        }
    }

    fn eat_string(&mut self) -> TokenKind {
        let mut chars = self.source[self.position..].chars();
        // Leading quote
        self.position += chars.next().map(char::len_utf8).unwrap_or(0);
        let mut kind = TokenKind::UnterminatedString;
        let mut is_escaped = false;
        self.eat_until(|ch| {
            if kind == TokenKind::String {
                return true;
            }
            if !is_escaped && ch == '"' {
                kind = TokenKind::String;
            }
            is_escaped = !is_escaped && ch == '\\';
            false
        });
        kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokenize(source: &str) -> Vec<(TokenKind, &str)> {
        let tokenizer = Tokenizer::new(source);
        tokenizer
            .map(|token| (token.kind, &source[token.span]))
            .collect()
    }

    #[test]
    fn empty_tokenizer_returns_none() {
        assert_eq!(tokenize(""), Vec::<(TokenKind, &str)>::new())
    }

    #[test]
    fn whitespace_returns_none() {
        assert_eq!(tokenize(" \n\t\r "), Vec::<(TokenKind, &str)>::new())
    }

    #[test]
    fn tokens_split_by_whitespace() {
        assert_eq!(
            tokenize("abc 123\nxyz\t42"),
            vec![
                (TokenKind::Identifier, "abc"),
                (TokenKind::Identifier, "123"),
                (TokenKind::Identifier, "xyz"),
                (TokenKind::Identifier, "42")
            ]
        )
    }

    #[test]
    fn parens_are_single_token() {
        assert_eq!(
            tokenize("(this() has) expressions()"),
            vec![
                (TokenKind::OpenParen, "("),
                (TokenKind::Identifier, "this"),
                (TokenKind::OpenParen, "("),
                (TokenKind::CloseParen, ")"),
                (TokenKind::Identifier, "has"),
                (TokenKind::CloseParen, ")"),
                (TokenKind::Identifier, "expressions"),
                (TokenKind::OpenParen, "("),
                (TokenKind::CloseParen, ")"),
            ],
        )
    }

    #[test]
    fn string_is_single_token() {
        assert_eq!(
            tokenize(r#""hello""#),
            vec![(TokenKind::String, r#""hello""#)]
        );
        assert_eq!(
            tokenize(r#"start "string is here" end"#),
            vec![
                (TokenKind::Identifier, "start"),
                (TokenKind::String, r#""string is here""#),
                (TokenKind::Identifier, "end")
            ]
        )
    }

    #[test]
    fn unterminated_string_takes_all() {
        assert_eq!(
            tokenize(r#"start "string is here with no end"#),
            vec![
                (TokenKind::Identifier, "start"),
                (
                    TokenKind::UnterminatedString,
                    r#""string is here with no end"#
                ),
            ]
        )
    }

    #[test]
    fn escaped_quote_does_not_end_string() {
        assert_eq!(
            tokenize(r#""hello \"world\"" next"#),
            vec![
                (TokenKind::String, r#""hello \"world\"""#),
                (TokenKind::Identifier, "next"),
            ]
        );
    }

    #[test]
    fn closing_quote_after_even_backslashes_ends_string() {
        for source in [r#""hello\\" next"#, r#""hello\\\\" next"#] {
            let string_end = source.find(" next").unwrap();
            assert_eq!(
                tokenize(source),
                vec![
                    (TokenKind::String, &source[..string_end]),
                    (TokenKind::Identifier, "next"),
                ]
            );
        }
    }
}
