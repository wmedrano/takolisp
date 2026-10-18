//! Parse takolisp source into heap-allocated values.
//!
//! This module provides [`Reader`] for reading expressions incrementally and
//! [`ReadVisitor`] for inspecting parsed values without materializing them.

use smallvec::SmallVec;

use crate::{
    heap::Heap,
    pair::Pair,
    tokenizer::{Span, TokenKind, Tokenizer},
    val::Val,
};

/// An error that occurs when parsing a string as a takolisp datastructure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadError {
    /// The kind of error.
    pub kind: ReadErrorKind,
    /// Where the error occurred.
    pub token_span: Span,
}

/// The kind of error that occurred.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadErrorKind {
    /// We reached an end paren without an opening paren.
    UnexpectedEndParen,
    /// An opening paren without a closing paren occurred.
    MissingEndParen,
    /// A malformed string was encountered. Usually this means it wasn't closed with a `"`.
    MalformedString,
}

/// Convert strings into takolisp datastructures.
///
/// Can also be used with a custom implementation to inspect the datastructure without materializing
/// it.
pub struct Reader<'a> {
    tokenizer: Tokenizer<'a>,
}

#[derive(Clone, PartialEq)]
pub enum Atom<'a> {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Symbol(&'a str),
    String(String),
}

impl<'a> Atom<'a> {
    pub fn to_val(self, heap: &mut Heap) -> Val {
        match self {
            Atom::Nil => Val::Nil,
            Atom::Bool(x) => x.into(),
            Atom::Int(x) => x.into(),
            Atom::Float(x) => x.into(),
            Atom::Symbol(sym) => heap.symbols.create_symbol(sym).into(),
            Atom::String(s) => heap.strings.insert(s).into(),
        }
    }
}

/// A trait that is used with [`Reader`] to inspect a string as Lisp datastructures.
pub trait ReadVisitor {
    /// Signals that a value has been visited. Note that this does not include lists.
    fn visit_atom(&mut self, span: Span, val: Atom);
    /// A list has been started.
    fn start_list(&mut self, span: Span);
    /// A list has just ended.
    fn end_list(&mut self, span: Span);
    /// A comment has been encountered.
    fn visit_comment(&mut self, span: Span, comment: &str);
}

impl<'a> Reader<'a> {
    /// Create a new reader on top of `source`.
    pub fn new(source: &'a str) -> Self {
        Reader {
            tokenizer: Tokenizer::new(source),
        }
    }

    /// Read the next item with `visitor`.
    ///
    /// Returns `None` if there are no more items.
    pub fn read(&mut self, visitor: &mut impl ReadVisitor) -> Option<Result<(), ReadError>> {
        loop {
            let (leading_kind, leading_token) = self.tokenizer.next_span()?;
            let res = match leading_kind {
                TokenKind::OpenParen => self.read_expr(leading_token, visitor),
                TokenKind::CloseParen => Err(ReadError {
                    kind: ReadErrorKind::UnexpectedEndParen,
                    token_span: leading_token,
                }),
                TokenKind::Identifier => {
                    let atom = self.parse_ident(self.tokenizer.source(leading_token.clone()));
                    visitor.visit_atom(leading_token, atom);
                    Ok(())
                }
                TokenKind::String => {
                    let atom = self.parse_string(self.tokenizer.source(leading_token.clone()));
                    visitor.visit_atom(leading_token, atom);
                    Ok(())
                }
                TokenKind::MalformedString => Err(ReadError {
                    kind: ReadErrorKind::MalformedString,
                    token_span: leading_token,
                }),
                TokenKind::Comment => {
                    visitor
                        .visit_comment(leading_token.clone(), self.tokenizer.source(leading_token));
                    continue;
                }
            };
            return Some(res);
        }
    }

    /// Read the next item as a `Val`.
    ///
    /// Returns `None` if there are no more items.
    pub fn read_val(&mut self, heap: &mut Heap) -> Option<Result<Val, ReadError>> {
        let mut visitor = ValReadVisitor::new(heap);
        match self.read(&mut visitor) {
            None => None,
            Some(Ok(())) => Some(Ok(visitor.singleton_val)),
            Some(Err(err)) => Some(Err(err)),
        }
    }

    fn read_expr(
        &mut self,
        start_span: Span,
        visitor: &mut impl ReadVisitor,
    ) -> Result<(), ReadError> {
        visitor.start_list(start_span.clone());
        let mut depth: u32 = 1;
        while let Some((kind, token_span)) = self.tokenizer.next_span() {
            match kind {
                TokenKind::OpenParen => {
                    visitor.start_list(token_span);
                    depth += 1;
                }
                TokenKind::CloseParen => {
                    visitor.end_list(token_span);
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                TokenKind::Identifier => {
                    let val = self.parse_ident(self.tokenizer.source(token_span.clone()));
                    visitor.visit_atom(token_span, val);
                }
                TokenKind::String => {
                    let val = self.parse_string(self.tokenizer.source(token_span.clone()));
                    visitor.visit_atom(token_span, val);
                }
                TokenKind::MalformedString => {
                    return Err(ReadError {
                        kind: ReadErrorKind::MalformedString,
                        token_span,
                    });
                }
                TokenKind::Comment => {
                    visitor.visit_comment(token_span.clone(), self.tokenizer.source(token_span));
                }
            }
        }
        Err(ReadError {
            kind: ReadErrorKind::MissingEndParen,
            token_span: start_span,
        })
    }

    fn parse_ident<'b>(&self, token: &'b str) -> Atom<'b> {
        match token {
            "#true" => return Atom::Bool(true),
            "#false" => return Atom::Bool(false),
            "nil" => return Atom::Nil,
            _ if is_numberlike(token) => {
                if let Ok(number) = token.parse::<i64>() {
                    return Atom::Int(number);
                }
                if let Ok(number) = token.parse::<f64>() {
                    return Atom::Float(number);
                }
            }
            _ => {}
        }
        Atom::Symbol(token)
    }

    fn parse_string(&self, token: &str) -> Atom<'_> {
        let expected_len = token.len().saturating_sub(2);
        let mut ret = String::with_capacity(expected_len);
        let text = &token[1..expected_len + 1];
        let mut escaped = false;
        for ch in text.chars() {
            if escaped {
                match ch {
                    '0' => ret.push('\0'),
                    'n' => ret.push('\n'),
                    't' => ret.push('\t'),
                    'r' => ret.push('\r'),
                    _ => ret.push(ch),
                }
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            ret.push(ch);
        }
        Atom::String(ret)
    }
}

fn is_numberlike(s: &str) -> bool {
    let b = s.as_bytes().first().copied().unwrap_or(0);
    b.is_ascii_digit() || b == b'-' || b == b'+' || b == b'.'
}

struct ValReadVisitor<'a> {
    singleton_val: Val,
    expr_stack: SmallVec<[Vec<Val>; 2]>,
    heap: &'a mut Heap,
}

impl<'a> ValReadVisitor<'a> {
    fn new(heap: &'a mut Heap) -> ValReadVisitor<'a> {
        ValReadVisitor {
            singleton_val: Val::Undefined,
            expr_stack: SmallVec::new(),
            heap,
        }
    }
}

impl<'a> ReadVisitor for ValReadVisitor<'a> {
    fn visit_atom(&mut self, _span: Span, atom: Atom) {
        match self.expr_stack.last_mut() {
            Some(vals) => {
                let val = match atom {
                    Atom::Nil => Val::Nil,
                    Atom::Bool(x) => x.into(),
                    Atom::Int(x) => x.into(),
                    Atom::Float(x) => x.into(),
                    Atom::Symbol(sym) => self.heap.symbols.create_symbol(sym).into(),
                    Atom::String(s) => self.heap.strings.insert(s).into(),
                };
                vals.push(val);
            }
            None => self.singleton_val = atom.to_val(self.heap),
        }
    }

    fn start_list(&mut self, _span: Span) {
        self.expr_stack.push(Vec::new());
    }

    fn end_list(&mut self, _span: Span) {
        let Some(vals) = self.expr_stack.pop() else {
            return;
        };
        let mut val = Val::Nil;
        for el in vals.iter().rev() {
            let pair = Pair { car: *el, cdr: val };
            val = Val::Pair(self.heap.pairs.insert(pair));
        }
        match self.expr_stack.last_mut() {
            Some(vals) => vals.push(val),
            None => self.singleton_val = val,
        }
    }

    fn visit_comment(&mut self, _span: Span, _comment: &str) {}
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;
    use crate::heap::Heap;

    /// Flattens a nil-terminated pair chain into a vector of values.
    fn list_items(heap: &Heap, val: Val) -> Vec<Val> {
        let mut items = Vec::new();
        let mut current = val;
        while let Val::Pair(handle) = current {
            let pair = *heap.pairs.get(handle).unwrap();
            items.push(pair.car);
            current = pair.cdr;
        }
        assert_eq!(current, Val::Nil, "expected a nil-terminated list");
        items
    }

    #[test]
    fn expressions_are_parsed_once() {
        let mut heap = Heap::default();
        let mut reader = Reader::new("#true #false");
        assert_eq!(reader.read_val(&mut heap), Some(Ok(Val::Bool(true))));
        assert_eq!(reader.read_val(&mut heap), Some(Ok(Val::Bool(false))));
        assert_eq!(reader.read_val(&mut heap), None);
        assert_eq!(reader.read_val(&mut heap), None);
    }

    #[test]
    fn booleans_are_parsed() {
        let mut heap = Heap::default();
        let mut reader = Reader::new("#true #false");
        assert_eq!(reader.read_val(&mut heap), Some(Ok(Val::Bool(true))));
        assert_eq!(reader.read_val(&mut heap), Some(Ok(Val::Bool(false))));
    }

    #[test]
    fn numbers_are_parsed() {
        let mut heap = Heap::default();
        let mut reader = Reader::new("1 2.0 -3 -4.5");
        assert_eq!(reader.read_val(&mut heap), Some(Ok(Val::Int(1))));
        assert_eq!(reader.read_val(&mut heap), Some(Ok(Val::Float(2.0))));
        assert_eq!(reader.read_val(&mut heap), Some(Ok(Val::Int(-3))));
        assert_eq!(reader.read_val(&mut heap), Some(Ok(Val::Float(-4.5))));
    }

    #[test]
    fn expressions_are_parsed() {
        let mut heap = Heap::default();
        let mut reader = Reader::new("(foo 1 2) (bar 3 (baz 4 5))");
        let foo = heap.symbols.create_symbol("foo");
        let bar = heap.symbols.create_symbol("bar");
        let baz = heap.symbols.create_symbol("baz");
        let first = reader.read_val(&mut heap).unwrap().unwrap();
        let Val::Pair(id) = first else {
            panic!("expected list")
        };
        assert_eq!(
            list_items(&heap, Val::Pair(id)),
            vec![Val::Symbol(foo), Val::Int(1), Val::Int(2)]
        );

        let second = reader.read_val(&mut heap).unwrap().unwrap();
        let Val::Pair(outer_id) = second else {
            panic!("expected list")
        };
        let outer = *heap.pairs.get(outer_id).unwrap();
        assert_eq!(outer.car, Val::Symbol(bar));
        let Val::Pair(rest_id) = outer.cdr else {
            panic!("expected rest of list")
        };
        let rest = *heap.pairs.get(rest_id).unwrap();
        assert_eq!(rest.car, Val::Int(3));
        // The third element cell holds the inner list value as its car.
        let Val::Pair(cell3_id) = rest.cdr else {
            panic!("expected third element")
        };
        let cell3 = *heap.pairs.get(cell3_id).unwrap();
        let Val::Pair(inner_id) = cell3.car else {
            panic!("expected inner list")
        };
        assert_eq!(
            list_items(&heap, Val::Pair(inner_id)),
            vec![Val::Symbol(baz), Val::Int(4), Val::Int(5)]
        );
    }

    #[test]
    fn unexpected_end_paren_is_error() {
        let mut heap = Heap::default();
        let mut reader = Reader::new("())");
        assert_matches!(reader.read_val(&mut heap), Some(Ok(_)));
        assert_eq!(
            reader.read_val(&mut heap),
            Some(Err(ReadError {
                kind: ReadErrorKind::UnexpectedEndParen,
                token_span: 2..3
            }))
        )
    }

    #[test]
    fn missing_end_paren_is_error() {
        let mut heap = Heap::default();
        let mut reader = Reader::new("(");
        assert_eq!(
            reader.read_val(&mut heap),
            Some(Err(ReadError {
                kind: ReadErrorKind::MissingEndParen,
                token_span: 0..1
            }))
        )
    }

    #[test]
    fn bad_string_is_error() {
        let mut heap = Heap::default();
        let mut reader = Reader::new("\"good string\" \" bad not closed\\\"");
        assert_matches!(reader.read_val(&mut heap), Some(Ok(_)));
        assert_eq!(
            reader.read_val(&mut heap),
            Some(Err(ReadError {
                kind: ReadErrorKind::MalformedString,
                token_span: 14..32
            }))
        )
    }

    #[test]
    fn strings_are_parsed_via_heap() {
        let mut heap = Heap::default();
        let mut reader = Reader::new("\"hello\" \"hi\\n\"");
        let first = reader.read_val(&mut heap).unwrap().unwrap();
        let Val::String(id) = first else {
            panic!("expected string")
        };
        assert_eq!(heap.strings.get(id).unwrap(), "hello");
        // mutable access
        heap.strings.get_mut(id).unwrap().push('!');
        assert_eq!(heap.strings.get(id).unwrap(), "hello!");

        let second = reader.read_val(&mut heap).unwrap().unwrap();
        let Val::String(id2) = second else {
            panic!("expected string")
        };
        assert_eq!(heap.strings.get(id2).unwrap(), "hi\n");
    }

    #[test]
    fn lists_mutable_via_heap() {
        let mut heap = Heap::default();
        let mut reader = Reader::new("(1 2)");
        let val = reader.read_val(&mut heap).unwrap().unwrap();
        let Val::Pair(id) = val else {
            panic!("expected list")
        };
        assert_eq!(list_items(&heap, val), vec![Val::Int(1), Val::Int(2)]);
        // Appending a value mutates the cdr of the last pair in the chain.
        let mut tail = id;
        while let Val::Pair(next) = heap.pairs.get(tail).unwrap().cdr {
            tail = next;
        }
        let appended = heap.pairs.insert(Pair {
            car: Val::Int(3),
            cdr: Val::Nil,
        });
        heap.pairs.get_mut(tail).unwrap().cdr = Val::Pair(appended);
        assert_eq!(
            list_items(&heap, val),
            vec![Val::Int(1), Val::Int(2), Val::Int(3)]
        );
    }

    #[test]
    fn comments_are_skipped() {
        let mut heap = Heap::default();
        let mut reader = Reader::new("; first\n(foo ; second\n 1) ; last");
        let val = reader.read_val(&mut heap).unwrap().unwrap();
        let Val::Pair(_) = val else {
            panic!("expected list")
        };
        let foo = heap.symbols.create_symbol("foo");
        assert_eq!(list_items(&heap, val), vec![Val::Symbol(foo), Val::Int(1)]);
        assert_eq!(reader.read_val(&mut heap), None);
    }

    #[test]
    fn comments_are_reported_to_visitors_inside_and_outside_lists() {
        #[derive(Default)]
        struct CommentVisitor(Vec<(Span, String)>);
        impl ReadVisitor for CommentVisitor {
            fn visit_atom(&mut self, _span: Span, _atom: Atom) {}
            fn start_list(&mut self, _span: Span) {}
            fn end_list(&mut self, _span: Span) {}
            fn visit_comment(&mut self, span: Span, comment: &str) {
                self.0.push((span, comment.to_owned()));
            }
        }

        let mut visitor = CommentVisitor::default();
        let mut reader = Reader::new(
            r#"; first
(foo ; second
 (bar ; third
 1)) ; last"#,
        );

        while reader.read(&mut visitor).is_some() {}

        assert_eq!(
            visitor.0,
            vec![
                (0..7, "; first".to_owned()),
                (13..21, "; second".to_owned()),
                (28..35, "; third".to_owned()),
                (41..47, "; last".to_owned()),
            ]
        );
    }
}
