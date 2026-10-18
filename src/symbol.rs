//! Symbols and symbol tables used by takolisp.
//!
//! [`Symbol`] values are compact and can represent short names inline. Longer
//! names and anonymous symbols are managed by [`SymbolTable`].

use std::{collections::HashMap, sync::Arc};

const INLINE_SIZE: usize = 7;

/// Contains a symbol that can be efficiently stored, hashed, and compared.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct Symbol(SymbolRepr);

/// Used to create (intern) and retrieve metadata from a `Symbol`.
#[derive(Default, Debug)]
pub struct SymbolTable {
    index_to_name: Vec<Arc<str>>,
    name_to_index: HashMap<Arc<str>, u32>,
    anon_id: u32,
}

impl Symbol {
    pub const BEGIN: Symbol = Symbol::must_inline("begin");
    pub const DEFINE: Symbol = Symbol::must_inline("define");
    pub const IF: Symbol = Symbol::must_inline("if");
    pub const LAMBDA: Symbol = Symbol::must_inline("lambda");
    pub const LET: Symbol = Symbol::must_inline("let");
    pub const RETURN: Symbol = Symbol::must_inline("return");
    pub const SET_B: Symbol = Symbol::must_inline("set!");
    pub const WHEN: Symbol = Symbol::must_inline("when");

    /// Create a `Symbol` for `s`.
    ///
    /// Panics if `s` is too long to store "inline". For a non-panicking version, use
    /// [`Symbol::try_from_inline`]. For longer symbols, use [`SymbolTable::create_symbol`].
    pub const fn must_inline(s: &str) -> Symbol {
        Symbol::try_from_inline(s).unwrap()
    }

    /// Create a `Symbol` for `s`.
    ///
    /// Returns `None` if `s` is too long to store "inline".
    pub const fn try_from_inline(s: &str) -> Option<Symbol> {
        let len = match InlineLen::try_from_len(s.len()) {
            Some(len) => len,
            None => return None,
        };
        let mut inline_bytes = [0u8; INLINE_SIZE];
        inline_bytes
            .split_at_mut(s.len())
            .0
            .copy_from_slice(s.as_bytes());
        Some(Symbol(SymbolRepr::Inline(len, inline_bytes)))
    }
}

impl SymbolTable {
    /// Create a new `symbol` from `s`.
    pub fn create_symbol(&mut self, s: &str) -> Symbol {
        if let Some(s) = Symbol::try_from_inline(s) {
            return s;
        }
        if let Some(id) = self.name_to_index.get(s) {
            return Symbol(SymbolRepr::Interned(*id));
        }
        let id = self.index_to_name.len() as u32;
        let s = Arc::<str>::from(s);
        self.index_to_name.push(s.clone());
        self.name_to_index.insert(s, id);
        Symbol(SymbolRepr::Interned(id))
    }

    /// Create a new anonymous symbol.
    ///
    /// Each returned value is unique.
    pub fn create_anonymous_symbol(&mut self) -> Symbol {
        self.anon_id += 1;
        Symbol(SymbolRepr::Anonymous(self.anon_id))
    }

    /// Get a type that can be used to display a symbol.
    pub fn display_symbol(&self, symbol: Symbol) -> impl '_ + std::fmt::Display {
        FormattedSymbol(self, symbol)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum InlineLen {
    Zero = 0,
    One = 1,
    Two = 2,
    Three = 3,
    Four = 4,
    Five = 5,
    Six = 6,
    Seven = 7,
}

impl InlineLen {
    const fn try_from_len(len: usize) -> Option<Self> {
        match len {
            0 => Some(Self::Zero),
            1 => Some(Self::One),
            2 => Some(Self::Two),
            3 => Some(Self::Three),
            4 => Some(Self::Four),
            5 => Some(Self::Five),
            6 => Some(Self::Six),
            7 => Some(Self::Seven),
            _ => None,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum SymbolRepr {
    Inline(InlineLen, [u8; INLINE_SIZE]),
    Interned(u32),
    Anonymous(u32),
}

struct FormattedSymbol<'a>(&'a SymbolTable, Symbol);

impl<'a> std::fmt::Display for FormattedSymbol<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.1.0 {
            SymbolRepr::Inline(len, bytes) => {
                let s = str::from_utf8(&bytes[0..len as usize]).unwrap_or("bad-symbol");
                f.write_str(s)
            }
            SymbolRepr::Interned(id) => match self.0.index_to_name.get(id as usize) {
                Some(s) => f.write_str(s),
                None => write!(f, "symbol-{id}"),
            },
            SymbolRepr::Anonymous(id) => write!(f, "anon-{id}"),
        }
    }
}

impl std::fmt::Debug for Symbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            SymbolRepr::Inline(len, bytes) => {
                let s = str::from_utf8(&bytes[..len as usize]).unwrap_or("bad-symbol");
                f.debug_tuple("Symbol").field(&s).finish()
            }
            repr => f.debug_tuple("Symbol").field(&repr).finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_is_small() {
        assert_eq!(std::mem::size_of::<Symbol>(), std::mem::size_of::<u64>());
    }

    #[test]
    fn symbol_display_string() {
        let mut table = SymbolTable::default();
        let cases = [
            "",
            "a",
            "1234567",
            "12345678",
            "this is a very long string that exceeds inline",
        ];
        for input in cases {
            let sym = table.create_symbol(input);
            assert_eq!(
                table.display_symbol(sym).to_string(),
                input,
                "display mismatch for {input:?}"
            );
        }
    }

    #[test]
    fn same_string_created_twice_returns_equal_symbol() {
        let mut table = SymbolTable::default();
        let short = "hi";
        let a = table.create_symbol(short);
        let b = table.create_symbol(short);
        assert_eq!(a, b);

        let long = "long_string_exceeds_inline_limit_12345";
        let c = table.create_symbol(long);
        let d = table.create_symbol(long);
        assert_eq!(c, d);

        let mut t1 = SymbolTable::default();
        let mut t2 = SymbolTable::default();
        let s1 = t1.create_symbol(short);
        let s2 = t2.create_symbol(short);
        assert_eq!(s1, s2);
    }

    #[test]
    fn different_strings_return_unequal_symbols() {
        let mut table = SymbolTable::default();
        let a = table.create_symbol("foo");
        let b = table.create_symbol("bar");
        assert_ne!(a, b);

        let long_a = "long_string_a_12345_exceeds_inline";
        let long_b = "long_string_b_12345_exceeds_inline";
        let c = table.create_symbol(long_a);
        let d = table.create_symbol(long_b);
        assert_ne!(c, d);
    }

    #[test]
    fn successive_anonymous_symbols_are_all_distinct() {
        let mut table = SymbolTable::default();
        let a = table.create_anonymous_symbol();
        let b = table.create_anonymous_symbol();
        let c = table.create_anonymous_symbol();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn anonymous_symbol_displays_with_anon_prefix() {
        let mut table = SymbolTable::default();
        let sym = table.create_anonymous_symbol();
        let displayed = table.display_symbol(sym).to_string();
        assert!(
            displayed.starts_with("anon-"),
            "expected anon- prefix, got {displayed:?}"
        );
    }

    #[test]
    fn anonymous_symbols_have_sequentially_incrementing_display_ids() {
        let mut table = SymbolTable::default();
        let a = table.create_anonymous_symbol();
        let b = table.create_anonymous_symbol();
        let c = table.create_anonymous_symbol();
        assert_eq!(table.display_symbol(a).to_string(), "anon-1");
        assert_eq!(table.display_symbol(b).to_string(), "anon-2");
        assert_eq!(table.display_symbol(c).to_string(), "anon-3");
    }

    #[test]
    fn interned_symbol_displays_original_text_from_owning_table() {
        let mut table = SymbolTable::default();
        let input = "long_string_exceeds_inline_limit_12345";
        let sym = table.create_symbol(input);
        assert_eq!(table.display_symbol(sym).to_string(), input);
    }

    #[test]
    fn try_from_inline_respects_byte_length_limit() {
        let table = SymbolTable::default();
        for input in ["", "a", "1234567"] {
            let sym = Symbol::try_from_inline(input)
                .unwrap_or_else(|| panic!("expected Some for {input:?}"));
            assert_eq!(
                table.display_symbol(sym).to_string(),
                input,
                "display mismatch for {input:?}"
            );
            assert_eq!(sym, Symbol::must_inline(input));
        }
        for input in ["12345678", "this is a very long string that exceeds inline"] {
            std::assert_matches!(
                Symbol::try_from_inline(input),
                None,
                "expected None for {input:?}"
            );
        }
    }

    #[test]
    fn same_string_via_try_from_inline_returns_equal_symbol() {
        let a = Symbol::try_from_inline("hi").unwrap();
        let b = Symbol::try_from_inline("hi").unwrap();
        assert_eq!(a, b);

        let c = Symbol::try_from_inline("abc").unwrap();
        let d = Symbol::try_from_inline("abc").unwrap();
        assert_eq!(c, d);
    }

    #[test]
    fn different_strings_via_try_from_inline_return_unequal_symbols() {
        let a = Symbol::try_from_inline("foo").unwrap();
        let b = Symbol::try_from_inline("bar").unwrap();
        assert_ne!(a, b);

        let c = Symbol::try_from_inline("abc").unwrap();
        let d = Symbol::try_from_inline("xyz").unwrap();
        assert_ne!(c, d);
    }

    #[test]
    fn byte_length_not_char_count_determines_try_from_inline() {
        // "🦀" is 4 bytes; 1 crab is inline (4 <=7), 2 crabs is 8 bytes >7
        assert_eq!("🦀".len(), 4);
        std::assert_matches!(Symbol::try_from_inline("🦀"), Some(_));
        assert_eq!(
            SymbolTable::default()
                .display_symbol(Symbol::try_from_inline("🦀").unwrap())
                .to_string(),
            "🦀"
        );

        assert_eq!("🦀🦀".len(), 8);
        std::assert_matches!(Symbol::try_from_inline("🦀🦀"), None);
    }

    #[test]
    fn debug_shows_inline_symbol_string() {
        assert_eq!(format!("{:?}", Symbol::must_inline("🦀")), "Symbol(\"🦀\")");
    }
}
