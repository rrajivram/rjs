use std::fmt;

use chumsky::{extra::ParserExtra, label::LabelError};

/// Displays each item preceded by a space, for the tail of an S-expression.
pub struct Spaced<'a, T>(pub &'a [T]);

impl<T: fmt::Display> fmt::Display for Spaced<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|item| write!(f, " {item}"))
    }
}

/// Displays the value, or `_` when absent.
pub struct OrBlank<'a, T>(pub &'a Option<T>);

impl<T: fmt::Display> fmt::Display for OrBlank<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(v) => write!(f, "{v}"),
            None => f.write_str("_"),
        }
    }
}

use crate::{
    combo::sequence,
    parser::{JSParser, TokenInput},
};

/// A non-empty, separator-interleaved sequence (`a, b, c` / `a.b.c`) that
/// keeps the separator tokens (and their spans) it was parsed with.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct Seq<T, S> {
    pub head: Box<T>,
    pub tail: Vec<(S, T)>,
}

impl<T, S> Seq<T, S> {
    pub fn items(&self) -> impl Iterator<Item = &T> {
        std::iter::once(&*self.head).chain(self.tail.iter().map(|(_, t)| t))
    }

    pub fn len(&self) -> usize {
        1 + self.tail.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn last(&self) -> &T {
        self.tail.last().map(|(_, t)| t).unwrap_or(&self.head)
    }
}

impl<'src, I, E, T, S, A> JSParser<'src, I, E, A> for Seq<T, S>
where
    I: TokenInput<'src>,
    T: JSParser<'src, I, E, A>,
    S: JSParser<'src, I, E, A>,
    E: ParserExtra<'src, I>,
    E::Error: LabelError<'src, I, String>,
    A: Clone,
{
    fn parser(args: A) -> impl chumsky::Parser<'src, I, Self, E> + Clone {
        sequence(T::parser(args.clone()), S::parser(args))
    }
}
