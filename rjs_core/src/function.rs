//! Functions, methods, arrow functions and classes — shared by the
//! expression and statement grammars.

use std::fmt;

use chumsky::{
    IterParser, Parser,
    extra::ParserExtra,
    label::LabelError,
    prelude::{any, choice, recursive},
};

use crate::{
    expr::{Expr, PropKey, prop_key},
    ident::Ident,
    keyword::{ASYNC, CLASS, EXTENDS, FUNCTION, GET, SET, STATIC},
    parser::{JSCtx, JSParser, TokenInput, oper, punct, same_line, semi},
    pattern::{Pattern, binding_element},
    statement::{BlockDisplay, Statement, block_body},
    token::{Operator, Punctuation, Token, TokenStruct},
    utils::Spaced,
};

#[derive(Debug, Clone)]
pub struct Function {
    pub name: Option<Ident>,
    pub is_async: bool,
    pub generator: bool,
    pub params: Vec<Pattern>,
    pub body: Vec<Statement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    Method,
    Get,
    Set,
}

/// An object-literal or class method. `func.name` is always `None`; the
/// name is `key`.
#[derive(Debug, Clone)]
pub struct Method {
    pub kind: MethodKind,
    pub key: PropKey,
    pub func: Function,
}

#[derive(Debug, Clone)]
pub struct Arrow {
    pub is_async: bool,
    pub params: Vec<Pattern>,
    pub body: ArrowBody,
}

#[derive(Debug, Clone)]
pub enum ArrowBody {
    Expr(Box<Expr>),
    Block(Vec<Statement>),
}

#[derive(Debug, Clone)]
pub struct Class {
    pub name: Option<Ident>,
    pub extends: Option<Expr>,
    pub members: Vec<ClassMember>,
}

#[derive(Debug, Clone)]
pub enum ClassMember {
    Method {
        is_static: bool,
        method: Method,
    },
    Field {
        is_static: bool,
        key: PropKey,
        value: Option<Expr>,
    },
}

/// `(a, b = 1, ...rest)`
pub(crate) fn params_parser<'src, I, E>(
    ctx: JSCtx<'src, I, E>,
) -> impl Parser<'src, I, Vec<Pattern>, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    choice((
        oper(Operator::Ellipsis)
            .ignore_then(ctx.pattern.clone())
            .map(|p| Pattern::Rest(Box::new(p))),
        binding_element(ctx),
    ))
    .separated_by(punct(Punctuation::Comma))
    .allow_trailing()
    .collect::<Vec<_>>()
    .delimited_by(
        punct(Punctuation::LeftParenthesis),
        punct(Punctuation::RightParenthesis),
    )
    .boxed()
}

/// Parameters followed by a `{ body }`.
fn function_tail<'src, I, E>(
    ctx: JSCtx<'src, I, E>,
) -> impl Parser<'src, I, (Vec<Pattern>, Vec<Statement>), E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    params_parser(ctx.clone()).then(block_body(ctx))
}

/// `async? function *? name? (params) { body }` — the name is mandatory for
/// declarations, optional for expressions.
pub(crate) fn function_parser<'src, I, E>(
    ctx: JSCtx<'src, I, E>,
    name_required: bool,
) -> impl Parser<'src, I, Function, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    let name = if name_required {
        Ident::parser(()).map(Some).boxed()
    } else {
        Ident::parser(()).or_not().boxed()
    };
    ASYNC::parser(())
        .then_ignore(same_line())
        .or_not()
        .then_ignore(FUNCTION::parser(()))
        .then(oper(Operator::Multiply).or_not())
        .then(name)
        .then(function_tail(ctx))
        .map(|(((is_async, star), name), (params, body))| Function {
            name,
            is_async: is_async.is_some(),
            generator: star.is_some(),
            params,
            body,
        })
        .boxed()
}

/// `key() {}`, `get key() {}`, `set key(v) {}`, `async key() {}`, `*key() {}`
pub(crate) fn method_parser<'src, I, E>(
    ctx: JSCtx<'src, I, E>,
) -> impl Parser<'src, I, Method, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    let key = prop_key(ctx.clone());
    let star = oper(Operator::Multiply).or_not().map(|s| s.is_some());
    // Each accessor/async prefix falls back to the next alternative when it
    // turns out to be the method's own name (`get() {}`, `async() {}`).
    let head = choice((
        GET::parser(())
            .ignore_then(key.clone())
            .map(|k| (MethodKind::Get, false, false, k)),
        SET::parser(())
            .ignore_then(key.clone())
            .map(|k| (MethodKind::Set, false, false, k)),
        ASYNC::parser(())
            .ignore_then(same_line())
            .ignore_then(star.clone())
            .then(key.clone())
            .map(|(g, k)| (MethodKind::Method, true, g, k)),
        star.then(key).map(|(g, k)| (MethodKind::Method, false, g, k)),
    ));
    head.then(function_tail(ctx))
        .map(|((kind, is_async, generator, key), (params, body))| Method {
            kind,
            key,
            func: Function {
                name: None,
                is_async,
                generator,
                params,
                body,
            },
        })
        .boxed()
}

/// `x => x`, `(a, b) => { ... }`, `async (a) => a`
pub(crate) fn arrow_parser<'src, I, E>(
    ctx: JSCtx<'src, I, E>,
) -> impl Parser<'src, I, Expr, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    // Only commit to parsing `(...)` as parameters when the matching `)` is
    // followed by `=>`. Otherwise every parenthesized expression would be
    // parsed twice (once as parameters with defaults), compounding
    // exponentially with nesting.
    let balanced = recursive(|balanced| {
        choice((
            punct(Punctuation::LeftParenthesis)
                .ignore_then(balanced.repeated())
                .then_ignore(punct(Punctuation::RightParenthesis))
                .ignored(),
            any()
                .filter(|t: &TokenStruct<'src>| {
                    !matches!(
                        t.token,
                        Token::Punctuation(
                            Punctuation::LeftParenthesis | Punctuation::RightParenthesis
                        )
                    )
                })
                .ignored(),
        ))
    });
    let arrow_ahead = punct(Punctuation::LeftParenthesis)
        .then(balanced.repeated())
        .then(punct(Punctuation::RightParenthesis))
        .then(oper(Operator::Arrow))
        .rewind();
    let params = choice((
        Ident::parser(()).map(|i| vec![Pattern::Ident(i)]),
        arrow_ahead.ignore_then(params_parser(ctx.clone())),
    ));
    let body = choice((
        block_body(ctx.clone()).map(ArrowBody::Block),
        ctx.expr.clone().map(|e| ArrowBody::Expr(Box::new(e))),
    ));
    let tail = params
        .then_ignore(same_line())
        .then_ignore(oper(Operator::Arrow))
        .then(body);
    choice((
        ASYNC::parser(())
            .ignore_then(same_line())
            .ignore_then(tail.clone())
            .map(|t| (true, t)),
        tail.map(|t| (false, t)),
    ))
    .map(|(is_async, (params, body))| {
        Expr::Arrow(Box::new(Arrow {
            is_async,
            params,
            body,
        }))
    })
    .boxed()
}

/// `class Name? (extends Base)? { members }`
pub(crate) fn class_parser<'src, I, E>(
    ctx: JSCtx<'src, I, E>,
    name_required: bool,
) -> impl Parser<'src, I, Class, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    let name = if name_required {
        Ident::parser(()).map(Some).boxed()
    } else {
        Ident::parser(()).or_not().boxed()
    };
    let is_static = STATIC::parser(()).or_not().map(|s| s.is_some());
    let method = is_static
        .clone()
        .then(method_parser(ctx.clone()))
        .map(|(is_static, method)| ClassMember::Method { is_static, method });
    let field = is_static
        .then(prop_key(ctx.clone()))
        .then(
            punct(Punctuation::Equals)
                .ignore_then(ctx.expr.clone())
                .or_not(),
        )
        .then_ignore(semi())
        .map(|((is_static, key), value)| ClassMember::Field {
            is_static,
            key,
            value,
        });
    let members = choice((
        punct(Punctuation::Semicolon).to(None),
        method.map(Some),
        field.map(Some),
    ))
    .repeated()
    .collect::<Vec<_>>()
    .map(|ms| ms.into_iter().flatten().collect::<Vec<_>>())
    .delimited_by(punct(Punctuation::LeftBrace), punct(Punctuation::RightBrace));

    CLASS::parser(())
        .ignore_then(name)
        .then(EXTENDS::parser(()).ignore_then(ctx.lhs.clone()).or_not())
        .then(members)
        .map(|((name, extends), members)| Class {
            name,
            extends,
            members,
        })
        .boxed()
}

/// `(a b=1 ...rest)`
struct Params<'a>(&'a [Pattern]);

impl fmt::Display for Params<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("(")?;
        for (i, p) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{p}")?;
        }
        f.write_str(")")
    }
}

/// `async ` / `*` decorations around a function-ish head word.
fn write_head(f: &mut fmt::Formatter<'_>, word: &str, func: &Function) -> fmt::Result {
    let async_ = if func.is_async { "async " } else { "" };
    let star = if func.generator { "*" } else { "" };
    write!(f, "{async_}{word}{star}")
}

impl fmt::Display for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("(")?;
        write_head(f, "function", self)?;
        if let Some(name) = &self.name {
            write!(f, " {name}")?;
        }
        write!(f, " {} {})", Params(&self.params), BlockDisplay(&self.body))
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let word = match self.kind {
            MethodKind::Method => "method",
            MethodKind::Get => "get",
            MethodKind::Set => "set",
        };
        f.write_str("(")?;
        write_head(f, word, &self.func)?;
        write!(
            f,
            " {} {} {})",
            self.key,
            Params(&self.func.params),
            BlockDisplay(&self.func.body)
        )
    }
}

impl fmt::Display for Arrow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let async_ = if self.is_async { "async " } else { "" };
        write!(f, "({async_}=> {} ", Params(&self.params))?;
        match &self.body {
            ArrowBody::Expr(e) => write!(f, "{e})"),
            ArrowBody::Block(b) => write!(f, "{})", BlockDisplay(b)),
        }
    }
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("(class")?;
        if let Some(name) = &self.name {
            write!(f, " {name}")?;
        }
        if let Some(base) = &self.extends {
            write!(f, " (extends {base})")?;
        }
        write!(f, "{})", Spaced(&self.members))
    }
}

impl fmt::Display for ClassMember {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (is_static, inner) = match self {
            ClassMember::Method { is_static, method } => (*is_static, method.to_string()),
            ClassMember::Field {
                is_static,
                key,
                value,
            } => (
                *is_static,
                match value {
                    Some(v) => format!("(field {key} {v})"),
                    None => format!("(field {key})"),
                },
            ),
        };
        if is_static {
            write!(f, "(static {inner})")
        } else {
            f.write_str(&inner)
        }
    }
}
