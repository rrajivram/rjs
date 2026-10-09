//! Identifiers: bare words that are not reserved keywords.

use std::fmt;

use chumsky::{Parser, extra::ParserExtra, label::LabelError};

use crate::{
    parser::{JSParser, TokenInput, token},
    span::TokenSpan,
    token::{Period, Token},
    utils::Seq,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ident {
    pub span: TokenSpan,
    pub value: String,
}

impl Ident {
    /// A property name (`a.if`, `{ class: 1 }`): any word, reserved or not.
    pub fn name_parser<'src, I, E>() -> impl Parser<'src, I, Ident, E> + Clone
    where
        I: TokenInput<'src>,
        E: ParserExtra<'src, I>,
        E::Error: LabelError<'src, I, String>,
    {
        token("property name", |t| match &t.token {
            Token::Word { raw, .. } => Some(Ident {
                span: t.span,
                value: (*raw).to_string(),
            }),
            _ => None,
        })
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.value)
    }
}

/// A possibly-qualified name: `t`, `schema.t`, `db.schema.t`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectName {
    pub parts: Seq<Ident, Period>,
}

impl ObjectName {
    pub fn span(&self) -> TokenSpan {
        TokenSpan {
            start: self.parts.head.span.start,
            end: self.parts.last().span.end,
        }
    }

    pub fn idents(&self) -> impl Iterator<Item = &Ident> {
        self.parts.items()
    }

    /// The dotted textual form, e.g. `schema.table`.
    pub fn to_dotted(&self) -> String {
        self.idents()
            .map(|i| i.value.as_str())
            .collect::<Vec<_>>()
            .join(".")
    }
}

impl<'src, I, E, A> JSParser<'src, I, E, A> for Ident
where
    I: TokenInput<'src>,
    E: ParserExtra<'src, I>,
    E::Error: LabelError<'src, I, String>,
{
    fn parser(_args: A) -> impl Parser<'src, I, Self, E> + Clone {
        token("identifier", |t| match &t.token {
            // A bare word is an identifier unless it's a *reserved* keyword;
            // contextual keywords (async, of, get, ...) are fine names.
            Token::Word { raw, keyword } if !keyword.is_some_and(|k| k.reserved()) => Some(Ident {
                span: t.span,
                value: (*raw).to_string(),
            }),
            _ => None,
        })
    }
}

impl<'src, I, E, A> JSParser<'src, I, E, A> for ObjectName
where
    I: TokenInput<'src>,
    E: ParserExtra<'src, I>,
    E::Error: LabelError<'src, I, String>,
    A: Clone,
{
    fn parser(args: A) -> impl Parser<'src, I, Self, E> + Clone {
        Seq::<Ident, Period>::parser(args).map(|parts| ObjectName { parts })
    }
}
