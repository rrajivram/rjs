use std::fmt;

use chumsky::{Parser, extra::ParserExtra, label::LabelError};
use either::Either;
use macros::JSParser;

use crate::{
    keyword,
    parser::{JSParser, TokenInput, token},
    span::TokenSpan,
    token::{StringStyle, Token},
};

#[derive(Debug, Clone, JSParser)]
pub enum Literal {
    Number(NumberLiteral),
    String(StringLiteral),
    Boolean(BooleanLiteral),
    Null(keyword::NULL),
}

#[derive(Debug, Clone)]
pub enum NumberValue {
    Int(i64),
    Float(f64),
}

#[derive(Debug, Clone)]
pub struct NumberLiteral {
    pub span: TokenSpan,
    pub raw: String,
    pub value: NumberValue,
}

#[derive(Debug, Clone)]
pub struct StringLiteral {
    pub span: TokenSpan,
    pub value: String,
}

#[derive(Debug, Clone, JSParser)]
pub struct BooleanLiteral(Either<keyword::TRUE, keyword::FALSE>);

impl BooleanLiteral {
    pub fn value(&self) -> bool {
        self.0.is_left()
    }

    pub fn span(&self) -> TokenSpan {
        match &self.0 {
            Either::Left(t) => t.span,
            Either::Right(f) => f.span,
        }
    }
}

impl Literal {
    pub fn span(&self) -> TokenSpan {
        match self {
            Literal::Number(n) => n.span,
            Literal::String(s) => s.span,
            Literal::Boolean(b) => b.span(),
            Literal::Null(n) => n.span,
        }
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Number(n) => f.write_str(&n.raw),
            Literal::String(s) => write!(f, "{:?}", s.value),
            Literal::Boolean(b) => write!(f, "{}", b.value()),
            Literal::Null(_) => f.write_str("null"),
        }
    }
}

impl<'src, I, E, A> JSParser<'src, I, E, A> for StringLiteral
where
    I: TokenInput<'src>,
    E: ParserExtra<'src, I>,
    E::Error: LabelError<'src, I, String>,
{
    fn parser(_args: A) -> impl Parser<'src, I, Self, E> + Clone {
        token("string literal", |t| match &t.token {
            Token::String {
                raw,
                kind: StringStyle::SingleQuoted(_) | StringStyle::DoubleQuoted(_),
            } => Some(StringLiteral {
                span: t.span,
                value: unescape(raw)?,
            }),
            _ => None,
        })
    }
}

/// Decode JS string escapes (`\n`, `\x41`, `A`, `\u{1F600}`, line
/// continuations). `None` for a malformed escape.
fn unescape(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'v' => out.push('\u{b}'),
            '0' => out.push('\0'),
            '\n' => {}
            'x' => {
                let hex: String = chars.by_ref().take(2).collect();
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            }
            'u' => {
                let unit = unicode_escape(&mut chars)?;
                // A UTF-16 surrogate pair spelled as two `\u` escapes.
                if (0xD800..0xDC00).contains(&unit) {
                    let mut lookahead = chars.clone();
                    if lookahead.next() == Some('\\')
                        && lookahead.next() == Some('u')
                        && let Some(low) = unicode_escape(&mut lookahead)
                            .filter(|low| (0xDC00..0xE000).contains(low))
                    {
                        chars = lookahead;
                        let combined = 0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
                        out.push(char::from_u32(combined)?);
                        continue;
                    }
                }
                out.push(char::from_u32(unit).unwrap_or('\u{FFFD}'));
            }
            other => out.push(other),
        }
    }
    Some(out)
}

/// The code unit after `\u`: either `XXXX` or `{X...}`.
fn unicode_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<u32> {
    let hex: String = if chars.peek() == Some(&'{') {
        chars.next();
        chars.by_ref().take_while(|c| *c != '}').collect()
    } else {
        chars.by_ref().take(4).collect()
    };
    u32::from_str_radix(&hex, 16).ok()
}

fn parse_number(raw: &str) -> Option<NumberValue> {
    for (prefix, radix) in [("0x", 16), ("0o", 8), ("0b", 2)] {
        if let Some(digits) = raw
            .get(..2)
            .filter(|p| p.eq_ignore_ascii_case(prefix))
            .map(|_| &raw[2..])
        {
            return i64::from_str_radix(digits, radix)
                .map(NumberValue::Int)
                .ok();
        }
    }
    match raw.parse::<i64>() {
        Ok(i) => Some(NumberValue::Int(i)),
        // not an i64 (has a '.', exponent, or overflows) — fall back to f64
        Err(_) => raw.parse::<f64>().ok().map(NumberValue::Float),
    }
}

impl<'src, I, E, A> JSParser<'src, I, E, A> for NumberLiteral
where
    I: TokenInput<'src>,
    E: ParserExtra<'src, I>,
    E::Error: LabelError<'src, I, String>,
{
    fn parser(_args: A) -> impl Parser<'src, I, Self, E> + Clone {
        token("number", |t| match &t.token {
            Token::Number { raw } => Some(NumberLiteral {
                span: t.span,
                raw: (*raw).to_string(),
                value: parse_number(raw)?,
            }),
            _ => None,
        })
    }
}
