#![allow(dead_code)]

use chumsky::{IterParser, Parser, error::Rich, extra, prelude::end};

use crate::{
    lexer::tokenize, parser::JSCtx, span::TokenSpan, statement::Statement, token::TokenStruct,
};

extern crate self as rjs_core;

pub mod combo;
pub mod expr;
pub mod function;
pub mod ident;
pub mod keyword;
pub mod lexer;
pub mod literal;
pub mod parser;
pub mod pattern;
pub mod span;
pub mod statement;
pub mod token;
pub mod utils;
pub mod validate;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    /// Byte range in the source text, when known.
    pub span: Option<TokenSpan>,
}

#[derive(Debug, Clone, Default)]
pub struct ParseResult {
    pub program: Vec<Statement>,
    pub diagnostics: Vec<String>,
    pub errors: Vec<String>,
}

const SOURCE: &str = "
import { readFile } from 'fs'

const add = (a, b = 1) => a + b
let { x, y: [first, ...rest] } = point

async function main(args) {
    for (let i = 0; i < args.length; i++) {
        if (args[i]?.startsWith('-')) continue
        const text = await readFile(args[i])
        console.log(text.length ?? 0)
    }
}

main(process.argv, extra)
const limit = 3
limit = 4
";

fn main() {
    match parse_js(SOURCE) {
        Ok(program) => {
            program.iter().for_each(|s| println!("{s}"));
            for d in validate::validate(&program) {
                match d.span {
                    Some(span) => println!("{}: {d}", line_col(SOURCE, span.start)),
                    None => println!("{d}"),
                }
            }
        }
        Err(errs) => errs.iter().for_each(|e| match e.span {
            Some(span) => println!("{}: error: {}", line_col(SOURCE, span.start), e.message),
            None => println!("error: {}", e.message),
        }),
    }
}

pub fn run_parser(src: &str) -> ParseResult {
    let mut program = vec![];
    let mut diagnostics = vec![];
    let mut errors = vec![];
    match parse_js(src) {
        Ok(prog) => {
            //program.iter().for_each(|s| println!("{s}"));
            program = prog;
            diagnostics = validate::validate(&program)
                .iter()
                .map(|d| match d.span {
                    Some(span) => format!("{}: {d}", line_col(SOURCE, span.start)),
                    None => format!("{d}"),
                })
                .collect::<Vec<_>>();
        }
        Err(errs) => {
            errors = errs
                .iter()
                .map(|e| match e.span {
                    Some(span) => format!("{}: error: {}", line_col(SOURCE, span.start), e.message),
                    None => format!("error: {}", e.message),
                })
                .collect::<Vec<_>>()
        }
    }
    ParseResult {
        program,
        diagnostics,
        errors,
    }
}

/// 1-based `line:column` of a byte offset.
fn line_col(src: &str, offset: usize) -> String {
    let before = &src[..offset.min(src.len())];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    format!("{line}:{col}")
}

fn parse_js(src: &str) -> Result<Vec<Statement>, Vec<ParseError>> {
    type TokInput<'src> = &'src [TokenStruct<'src>];
    type TokExtra<'src> = extra::Err<Rich<'src, TokenStruct<'src>>>;

    let tokens = tokenize(src).map_err(|errs| {
        errs.into_iter()
            .map(|e| ParseError {
                message: e.to_string(),
                span: Some(TokenSpan::from(*e.span())),
            })
            .collect::<Vec<_>>()
    })?;
    JSCtx::<TokInput, TokExtra>::build()
        .stmt
        .repeated()
        .collect::<Vec<_>>()
        .then_ignore(end())
        .parse(&tokens[..])
        .into_result()
        .map_err(|errs| {
            errs.into_iter()
                .map(|e| ParseError {
                    message: e.to_string(),
                    // Parser spans index into the token list; map back to
                    // source bytes.
                    span: tokens.get(e.span().start).map(|t| t.span),
                })
                .collect()
        })
}
