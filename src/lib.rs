use core::range::Range;

pub struct Vm {}

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

    fn eat_until(&mut self, pred: impl Fn(char) -> bool) {
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
        self.eat_until(|ch| ch.is_whitespace());
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
}
