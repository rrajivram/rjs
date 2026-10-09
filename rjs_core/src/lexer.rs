use chumsky::{
    IterParser, Parser,
    error::Rich,
    extra,
    prelude::{any, choice, end, just, one_of},
};

use crate::{
    keyword::Keyword,
    span::TokenSpan,
    token::{Operator, Punctuation, StringStyle, Token, TokenStruct},
};

type LexError<'src> = extra::Err<Rich<'src, char>>;

fn tok(token: Token<'_>, span: chumsky::span::SimpleSpan) -> TokenStruct<'_> {
    TokenStruct {
        token,
        span: TokenSpan::from(span),
        newline_before: false,
    }
}

fn word<'src>() -> impl Parser<'src, &'src str, TokenStruct<'src>, LexError<'src>> + Clone {
    let start = any().filter(|c: &char| c.is_alphabetic() || *c == '_' || *c == '$');
    let rest = any().filter(|c: &char| c.is_alphanumeric() || *c == '_' || *c == '$');
    start
        .then(rest.repeated())
        .to_slice()
        .map_with(|s: &str, e| {
            tok(
                Token::Word {
                    raw: s,
                    keyword: Keyword::get(s),
                },
                e.span(),
            )
        })
}

fn number<'src>() -> impl Parser<'src, &'src str, TokenStruct<'src>, LexError<'src>> + Clone {
    let digits = |radix: u32| {
        any()
            .filter(move |c: &char| c.is_digit(radix))
            .repeated()
            .at_least(1)
    };
    // `0xFF`, `0o17`, `0b101`
    let prefixed = just('0')
        .then(choice((
            one_of("xX").then(digits(16)).ignored(),
            one_of("oO").then(digits(8)).ignored(),
            one_of("bB").then(digits(2)).ignored(),
        )))
        .ignored();
    // `123`, `1.5`, `1.`, `.5`, `1e-3` — no sign (unary minus belongs to the
    // expression parser, otherwise `1-2` would lex as two numbers).
    let exponent = one_of("eE").then(one_of("+-").or_not()).then(digits(10));
    let decimal = choice((
        digits(10)
            .then(just('.').then(digits(10).or_not()).or_not())
            .ignored(),
        just('.').then(digits(10)).ignored(),
    ))
    .then(exponent.or_not())
    .ignored();
    choice((prefixed, decimal))
        .to_slice()
        .map_with(|s: &str, e| tok(Token::Number { raw: s }, e.span()))
}

fn string<'src>() -> impl Parser<'src, &'src str, TokenStruct<'src>, LexError<'src>> + Clone {
    // The raw slice keeps backslash escapes; they're decoded at literal-parse
    // time. An escaped quote must be consumed here so it can't end the string.
    let quoted = |quote: char| {
        choice((
            just('\\').then(any()).ignored(),
            any()
                .filter(move |c: &char| *c != quote && *c != '\\' && *c != '\n')
                .ignored(),
        ))
        .repeated()
        .to_slice()
        .delimited_by(just(quote), just(quote))
    };
    choice((
        quoted('\'').map_with(|s: &str, e| {
            tok(
                Token::String {
                    raw: s,
                    kind: StringStyle::SingleQuoted(Some('\'')),
                },
                e.span(),
            )
        }),
        quoted('"').map_with(|s: &str, e| {
            tok(
                Token::String {
                    raw: s,
                    kind: StringStyle::DoubleQuoted(Some('"')),
                },
                e.span(),
            )
        }),
    ))
}

/// `a?.5:b` is a conditional, not optional chaining: `?` followed by `.digit`
/// lexes as a lone `?`.
fn question_before_digit<'src>()
-> impl Parser<'src, &'src str, TokenStruct<'src>, LexError<'src>> + Clone {
    just('?')
        .then_ignore(
            just('.')
                .then(any().filter(|c: &char| c.is_ascii_digit()))
                .rewind(),
        )
        .map_with(|_, e| tok(Token::Punctuation(Punctuation::QuestionMark), e.span()))
}

fn operator<'src>() -> impl Parser<'src, &'src str, TokenStruct<'src>, LexError<'src>> + Clone {
    Operator::all_parser().map_with(|op, e| tok(Token::Operator(op), e.span()))
}

fn punctuation<'src>() -> impl Parser<'src, &'src str, TokenStruct<'src>, LexError<'src>> + Clone {
    any().try_map(|c: char, span| match Punctuation::from_char(c) {
        Some(p) => Ok(tok(Token::Punctuation(p), span)),
        None => Err(Rich::custom(span, format!("'{c}' is not punctuation"))),
    })
}

fn whitespace<'src>() -> impl Parser<'src, &'src str, TokenStruct<'src>, LexError<'src>> + Clone {
    any()
        .filter(|c: &char| c.is_whitespace())
        .repeated()
        .at_least(1)
        .map_with(|(), e| tok(Token::Space, e.span()))
}

fn comment<'src>() -> impl Parser<'src, &'src str, TokenStruct<'src>, LexError<'src>> + Clone {
    let line = just("//")
        .then(any().filter(|c: &char| *c != '\n').repeated())
        .ignored();
    let block = any()
        .and_is(just("*/").not())
        .repeated()
        .delimited_by(just("/*"), just("*/"))
        .ignored();
    line.or(block).map_with(|(), e| tok(Token::Space, e.span()))
}

pub fn lexer<'src>() -> impl Parser<'src, &'src str, Vec<TokenStruct<'src>>, LexError<'src>> {
    // Order matters: comments before operators (`/` vs `//`), operators
    // before punctuation (`<=` vs `<`), numbers before punctuation (`.5`).
    choice((
        comment(),
        question_before_digit(),
        operator(),
        number(),
        word(),
        string(),
        whitespace(),
        punctuation(),
    ))
    .repeated()
    .collect()
    .then_ignore(end())
}

/// Lex `src` into tokens with whitespace and comments removed — the input the
/// statement parsers expect. Each token records whether a line break preceded
/// it.
pub fn tokenize(src: &str) -> Result<Vec<TokenStruct<'_>>, Vec<Rich<'_, char>>> {
    let tokens = lexer().parse(src).into_result()?;
    let mut out = Vec::with_capacity(tokens.len());
    let mut newline = false;
    for mut t in tokens {
        if t.token == Token::Space {
            newline |= src
                .get(t.span.start..t.span.end)
                .is_some_and(|s| s.contains('\n'));
        } else {
            t.newline_before = newline;
            newline = false;
            out.push(t);
        }
    }
    Ok(out)
}
