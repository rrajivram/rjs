//! Binding patterns: the targets of declarations, parameters and `catch`
//! clauses — identifiers plus array/object destructuring.

use std::fmt;

use chumsky::{IterParser, Parser, extra::ParserExtra, label::LabelError, prelude::choice};

use crate::{
    expr::{Expr, PropKey, prop_key},
    ident::Ident,
    parser::{JSCtx, JSParser, TokenInput, oper, punct},
    token::{Operator, Punctuation},
};

#[derive(Debug, Clone)]
/// A binding target: where a value goes when it's bound to a variable, in
/// declarations (`let <pattern> = ...`), function parameters, `catch (...)`
/// and `for (const <pattern> of ...)`.
///
/// Usually just a name, but it can also be a *destructuring* pattern that
/// pulls a value apart into several variables at once:
///
/// ```js
/// const { name, age } = user;    // Object: two variables from properties
/// const [first, ...others] = xs; // Array: from elements, rest into `others`
/// function f({ id = 0 }) {}      // works in parameters too, with defaults
/// ```
pub enum Pattern {
    /// A single variable: the `x` in `let x = 1`.
    Ident(Ident),
    /// Array destructuring, binding by position:
    /// `const [a, b] = [1, 2]` sets `a = 1`, `b = 2`.
    ///
    /// `None` is a hole that skips an element: `const [, second] = xs`.
    Array(Vec<Option<Pattern>>),
    /// Object destructuring, binding by property name:
    /// `const { x, y: renamed } = point` sets `x = point.x` and
    /// `renamed = point.y`. See [`PatternProp`].
    Object(Vec<PatternProp>),
    /// `target = default`: uses `default` when the value is `undefined`,
    /// e.g. the `b = 2` in `function f(a, b = 2)` or `const [x = 0] = xs`.
    Default(Box<Pattern>, Box<Expr>),
    /// `...target`: collects the remaining elements into an array, as the
    /// last array-pattern element or last parameter: `function f(...args)`.
    Rest(Box<Pattern>),
}

#[derive(Debug, Clone)]
/// One entry of an object destructuring pattern `{ ... }`.
pub enum PatternProp {
    /// `key: pattern`: binds property `key` to a differently named variable
    /// or a nested pattern: `{ id: userId }`, `{ pos: { x, y } }`.
    KeyValue(PropKey, Pattern),
    /// `name`: binds property `name` to a variable of the same name.
    /// `name = default` also supplies a fallback: `{ port = 80 }`.
    Shorthand(Ident, Option<Expr>),
    /// `...rest`: collects the remaining properties into a new object:
    /// `const { id, ...fields } = record`.
    Rest(Pattern),
}

pub(crate) fn pattern_body<'src, I, E>(
    ctx: JSCtx<'src, I, E>,
) -> impl Parser<'src, I, Pattern, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    let comma = punct(Punctuation::Comma);
    let elem = binding_element(ctx.clone());
    let rest = oper(Operator::Ellipsis).ignore_then(ctx.pattern.clone());

    let array = choice((rest.clone().map(|p| Pattern::Rest(Box::new(p))), elem.clone()))
        .or_not()
        .separated_by(comma.clone())
        .collect::<Vec<_>>()
        .delimited_by(
            punct(Punctuation::LeftBracket),
            punct(Punctuation::RightBracket),
        )
        .map(|elems| Pattern::Array(drop_trailing_hole(elems)));

    let prop = choice((
        rest.map(PatternProp::Rest),
        prop_key(ctx.clone())
            .then_ignore(punct(Punctuation::Colon))
            .then(elem)
            .map(|(key, value)| PatternProp::KeyValue(key, value)),
        Ident::parser(())
            .then(
                punct(Punctuation::Equals)
                    .ignore_then(ctx.expr.clone())
                    .or_not(),
            )
            .map(|(name, default)| PatternProp::Shorthand(name, default)),
    ));
    let object = prop
        .separated_by(comma)
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(punct(Punctuation::LeftBrace), punct(Punctuation::RightBrace))
        .map(Pattern::Object);

    choice((Ident::parser(()).map(Pattern::Ident), array, object)).boxed()
}

/// A pattern with an optional `= default`, as in parameters and nested
/// destructuring.
pub(crate) fn binding_element<'src, I, E>(
    ctx: JSCtx<'src, I, E>,
) -> impl Parser<'src, I, Pattern, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    ctx.pattern
        .clone()
        .then(
            punct(Punctuation::Equals)
                .ignore_then(ctx.expr.clone())
                .or_not(),
        )
        .map(|(target, default)| match default {
            Some(d) => Pattern::Default(Box::new(target), Box::new(d)),
            None => target,
        })
}

/// `[a,]` has one element, not a trailing hole; `[a,,]` has one hole.
pub(crate) fn drop_trailing_hole<T>(mut elems: Vec<Option<T>>) -> Vec<Option<T>> {
    if matches!(elems.last(), Some(None)) {
        elems.pop();
    }
    elems
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Pattern::Ident(i) => write!(f, "{i}"),
            Pattern::Array(elems) => {
                f.write_str("[")?;
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" ")?;
                    }
                    match e {
                        Some(p) => write!(f, "{p}")?,
                        None => f.write_str("_")?,
                    }
                }
                f.write_str("]")
            }
            Pattern::Object(props) => {
                f.write_str("{")?;
                for (i, p) in props.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" ")?;
                    }
                    write!(f, "{p}")?;
                }
                f.write_str("}")
            }
            Pattern::Default(target, default) => write!(f, "{target}={default}"),
            Pattern::Rest(target) => write!(f, "...{target}"),
        }
    }
}

impl fmt::Display for PatternProp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PatternProp::KeyValue(key, value) => write!(f, "{key}:{value}"),
            PatternProp::Shorthand(name, None) => write!(f, "{name}"),
            PatternProp::Shorthand(name, Some(default)) => write!(f, "{name}={default}"),
            PatternProp::Rest(target) => write!(f, "...{target}"),
        }
    }
}
