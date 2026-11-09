use core::range::Range;

pub type Span = Range<usize>;

pub struct Tokenizer<'a> {
    position: usize,
    source: &'a str,
}

impl<'a> Tokenizer<'a> {
    pub fn new(source: &'a str) -> Self {
        Tokenizer {
            position: 0,
            source,
        }
    }

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

    fn eat_next(&mut self) {
        let Some(leading_ch) = self.source[self.position..].chars().next() else {
            return;
        };
        match leading_ch {
            '"' => self.eat_string(),
            '(' | ')' => self.position += leading_ch.len_utf8(),
            _ => self.eat_until(|ch| ch.is_whitespace() || matches!(ch, '(' | ')' | '"')),
        }
    }

    fn eat_string(&mut self) {
        let mut chars = (&self.source[self.position..]).chars();
        // Leading quote
        self.position += chars.next().map(char::len_utf8).unwrap_or(0);
        let mut is_done = false;
        let mut is_escaped = false;
        self.eat_until(|ch| {
            if is_done {
                return true;
            }
            is_done = !is_escaped && ch == '"';
            is_escaped = !is_escaped && ch == '\\';
            false
        });
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Span;

    fn next(&mut self) -> Option<Span> {
        self.eat_whitespace();
        let start = self.position;
        self.eat_next();
        let end = self.position;
        (start != end).then(|| (start..end).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokenize(source: &str) -> Vec<&str> {
        let tokenizer = Tokenizer::new(source);
        tokenizer.map(|span| &source[span]).collect()
    }

    #[test]
    fn empty_tokenizer_returns_none() {
        assert_eq!(tokenize(""), Vec::<&str>::new())
    }

    #[test]
    fn whitespace_returns_none() {
        assert_eq!(tokenize(" \n\t\r "), Vec::<&str>::new())
    }

    #[test]
    fn tokens_split_by_whitespace() {
        assert_eq!(
            tokenize("abc 123\nxyz\t42"),
            vec!["abc", "123", "xyz", "42"]
        )
    }

    #[test]
    fn parens_are_single_token() {
        assert_eq!(
            tokenize("(this() has) expressions()"),
            vec!["(", "this", "(", ")", "has", ")", "expressions", "(", ")"],
        )
    }

    #[test]
    fn string_is_single_token() {
        assert_eq!(
            tokenize(r#"start "string is here" end"#),
            vec!["start", r#""string is here""#, "end"]
        )
    }

    #[test]
    fn unterminated_string_takes_all() {
        assert_eq!(
            tokenize(r#"start "string is here with no end"#),
            vec!["start", r#""string is here with no end"#]
        )
    }
}
