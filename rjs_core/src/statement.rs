use std::fmt;

use chumsky::{IterParser, Parser, extra::ParserExtra, label::LabelError, prelude::choice};
use macros::JSParser;

use crate::{
    expr::{Expr, seq_expr},
    function::{Class, Function, class_parser, function_parser},
    ident::Ident,
    keyword::{
        AS, AWAIT, BREAK, CASE, CATCH, CONST, CONTINUE, DEBUGGER, DEFAULT, DO, ELSE, EXPORT,
        FINALLY, FOR, FROM, IF, IMPORT, IN, LET, OF, RETURN, SWITCH, THROW, TRY, VAR, WHILE,
    },
    literal::StringLiteral,
    parser::{JSCtx, JSParser, TokenInput, oper, punct, same_line, semi},
    pattern::Pattern,
    span::TokenSpan,
    token::{Operator, Punctuation},
    utils::{OrBlank, Spaced},
};

#[derive(Debug, Clone)]
pub enum Statement {
    Empty,
    Block(Vec<Statement>),
    Expr(Expr),
    VarDecl(VarDecl),
    Function(Box<Function>),
    Class(Box<Class>),
    If {
        test: Expr,
        then: Box<Statement>,
        otherwise: Option<Box<Statement>>,
    },
    For {
        init: Option<ForInit>,
        test: Option<Expr>,
        update: Option<Expr>,
        body: Box<Statement>,
    },
    ForIn {
        left: ForLeft,
        right: Expr,
        body: Box<Statement>,
    },
    ForOf {
        is_await: bool,
        left: ForLeft,
        right: Expr,
        body: Box<Statement>,
    },
    While {
        test: Expr,
        body: Box<Statement>,
    },
    DoWhile {
        body: Box<Statement>,
        test: Expr,
    },
    Return {
        span: TokenSpan,
        value: Option<Expr>,
    },
    Break {
        span: TokenSpan,
        label: Option<Ident>,
    },
    Continue {
        span: TokenSpan,
        label: Option<Ident>,
    },
    Throw(Expr),
    Try {
        block: Vec<Statement>,
        handler: Option<Catch>,
        finalizer: Option<Vec<Statement>>,
    },
    Switch {
        discriminant: Expr,
        cases: Vec<SwitchCase>,
    },
    Labeled {
        label: Ident,
        body: Box<Statement>,
    },
    Debugger,
    Import(ImportDecl),
    Export {
        span: TokenSpan,
        decl: ExportDecl,
    },
}

#[derive(Debug, Clone, JSParser)]
pub enum DeclKind {
    Let(LET),
    Const(CONST),
    Var(VAR),
}

#[derive(Debug, Clone)]
pub struct VarDecl {
    pub kind: DeclKind,
    pub decls: Vec<Declarator>,
}

#[derive(Debug, Clone)]
pub struct Declarator {
    pub target: Pattern,
    pub init: Option<Expr>,
}

#[derive(Debug, Clone)]
pub enum ForInit {
    VarDecl(VarDecl),
    Expr(Expr),
}

/// The binding of a `for-in`/`for-of` loop.
#[derive(Debug, Clone)]
pub enum ForLeft {
    Decl(DeclKind, Pattern),
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub struct Catch {
    pub param: Option<Pattern>,
    pub body: Vec<Statement>,
}

/// `case test:` or, when `test` is `None`, `default:`.
#[derive(Debug, Clone)]
pub struct SwitchCase {
    /// The `case`/`default` keyword.
    pub span: TokenSpan,
    pub test: Option<Expr>,
    pub body: Vec<Statement>,
}

#[derive(Debug, Clone)]
pub struct ImportDecl {
    /// The `import` keyword.
    pub span: TokenSpan,
    pub default: Option<Ident>,
    pub namespace: Option<Ident>,
    pub named: Vec<ModuleSpec>,
    pub source: String,
}

/// `name` or `name as alias` in an import/export list.
#[derive(Debug, Clone)]
pub struct ModuleSpec {
    pub name: Ident,
    pub alias: Option<Ident>,
}

#[derive(Debug, Clone)]
pub enum ExportDecl {
    /// `export const/function/class ...`
    Decl(Box<Statement>),
    Default(Expr),
    /// `export { a, b as c } (from 'm')?`
    Named {
        specs: Vec<ModuleSpec>,
        source: Option<String>,
    },
    /// `export * (as ns)? from 'm'`
    All {
        alias: Option<Ident>,
        source: String,
    },
}

impl<'src, I, E> JSParser<'src, I, E, JSCtx<'src, I, E>> for Statement
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    fn parser(args: JSCtx<'src, I, E>) -> impl Parser<'src, I, Self, E> + Clone {
        args.stmt
    }
}

/// `{ statements }`
pub(crate) fn block_body<'src, I, E>(
    ctx: JSCtx<'src, I, E>,
) -> impl Parser<'src, I, Vec<Statement>, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    ctx.stmt
        .clone()
        .repeated()
        .collect::<Vec<_>>()
        .delimited_by(punct(Punctuation::LeftBrace), punct(Punctuation::RightBrace))
        .boxed()
}

/// `let a = 1, [b] = c` without the terminator, so `for` heads can reuse it.
fn var_decl<'src, I, E>(ctx: JSCtx<'src, I, E>) -> impl Parser<'src, I, VarDecl, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    let declarator = ctx
        .pattern
        .clone()
        .then(
            punct(Punctuation::Equals)
                .ignore_then(ctx.expr.clone())
                .or_not(),
        )
        .map(|(target, init)| Declarator { target, init });
    DeclKind::parser(ctx.clone())
        .then(
            declarator
                .separated_by(punct(Punctuation::Comma))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .map(|(kind, decls)| VarDecl { kind, decls })
        .boxed()
}

fn module_parts<'src, I, E>() -> (
    impl Parser<'src, I, String, E> + Clone,
    impl Parser<'src, I, Vec<ModuleSpec>, E> + Clone,
)
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    let source = StringLiteral::parser(()).map(|s| s.value);
    let spec = Ident::name_parser()
        .then(AS::parser(()).ignore_then(Ident::name_parser()).or_not())
        .map(|(name, alias)| ModuleSpec { name, alias });
    let named = spec
        .separated_by(punct(Punctuation::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(punct(Punctuation::LeftBrace), punct(Punctuation::RightBrace));
    (source, named)
}

fn import_parser<'src, I, E>() -> impl Parser<'src, I, Statement, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    let (source, named) = module_parts();
    let namespace = oper(Operator::Multiply)
        .ignore_then(AS::parser(()))
        .ignore_then(Ident::parser(()));
    let decl = |default, namespace, named| ImportDecl {
        span: TokenSpan { start: 0, end: 0 },
        default,
        namespace,
        named,
        source: String::new(),
    };
    // `x`, `x, * as ns`, `x, { a }`, `* as ns`, `{ a, b as c }`
    let clause = choice((
        Ident::parser(())
            .then(
                punct(Punctuation::Comma)
                    .ignore_then(choice((
                        namespace.clone().map(|ns| (Some(ns), Vec::new())),
                        named.clone().map(|n| (None, n)),
                    )))
                    .or_not(),
            )
            .map(move |(default, rest)| {
                let (namespace, named) = rest.unwrap_or_default();
                decl(Some(default), namespace, named)
            }),
        namespace.map(move |ns| decl(None, Some(ns), Vec::new())),
        named.map(move |named| decl(None, None, named)),
    ));
    IMPORT::parser(())
        .then(choice((
            source.clone().map(move |source| ImportDecl {
                source,
                ..decl(None, None, Vec::new())
            }),
            clause
                .then_ignore(FROM::parser(()))
                .then(source)
                .map(|(decl, source)| ImportDecl { source, ..decl }),
        )))
        .then_ignore(semi())
        .map(|(kw, decl)| Statement::Import(ImportDecl { span: kw.span, ..decl }))
        .boxed()
}

fn export_parser<'src, I, E>(ctx: JSCtx<'src, I, E>) -> impl Parser<'src, I, Statement, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    let (source, named) = module_parts();
    let default = DEFAULT::parser(())
        .ignore_then(choice((
            function_parser(ctx.clone(), false).map(|f| Expr::Function(Box::new(f))),
            class_parser(ctx.clone(), false).map(|c| Expr::Class(Box::new(c))),
            ctx.expr.clone().then_ignore(semi()),
        )))
        .map(ExportDecl::Default);
    let all = oper(Operator::Multiply)
        .ignore_then(AS::parser(()).ignore_then(Ident::name_parser()).or_not())
        .then_ignore(FROM::parser(()))
        .then(source.clone())
        .then_ignore(semi())
        .map(|(alias, source)| ExportDecl::All { alias, source });
    let list = named
        .then(FROM::parser(()).ignore_then(source).or_not())
        .then_ignore(semi())
        .map(|(specs, source)| ExportDecl::Named { specs, source });
    let decl = choice((
        var_decl(ctx.clone())
            .then_ignore(semi())
            .map(Statement::VarDecl),
        function_parser(ctx.clone(), true).map(|f| Statement::Function(Box::new(f))),
        class_parser(ctx, true).map(|c| Statement::Class(Box::new(c))),
    ))
    .map(|s| ExportDecl::Decl(Box::new(s)));
    EXPORT::parser(())
        .then(choice((default, all, list, decl)))
        .map(|(kw, decl)| Statement::Export {
            span: kw.span,
            decl,
        })
        .boxed()
}

pub(crate) fn stmt_body<'src, I, E>(
    ctx: JSCtx<'src, I, E>,
) -> impl Parser<'src, I, Statement, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    let stmt = ctx.stmt.clone().map(Box::new);
    let seq = seq_expr(ctx.clone());
    let paren_seq = seq.clone().delimited_by(
        punct(Punctuation::LeftParenthesis),
        punct(Punctuation::RightParenthesis),
    );
    let block = block_body(ctx.clone());
    let decl = var_decl(ctx.clone());
    let semicolon = punct(Punctuation::Semicolon);
    // `break label` / `continue label`: the label must be on the same line.
    let label = same_line().ignore_then(Ident::parser(())).or_not();

    let if_stmt = IF::parser(())
        .ignore_then(paren_seq.clone())
        .then(stmt.clone())
        .then(ELSE::parser(()).ignore_then(stmt.clone()).or_not())
        .map(|((test, then), otherwise)| Statement::If {
            test,
            then,
            otherwise,
        })
        .boxed();

    // Tried before the C-style loop; the head is cheap to re-parse when it
    // turns out to be `for (init; ...)`.
    let for_left = choice((
        DeclKind::parser(ctx.clone())
            .then(ctx.pattern.clone())
            .map(|(kind, target)| ForLeft::Decl(kind, target)),
        ctx.lhs.clone().map(ForLeft::Expr),
    ));
    let for_in_of = FOR::parser(())
        .ignore_then(AWAIT::parser(()).or_not())
        .then_ignore(punct(Punctuation::LeftParenthesis))
        .then(for_left)
        .then(choice((
            IN::parser(()).ignore_then(seq.clone()).map(|e| (false, e)),
            OF::parser(()).ignore_then(ctx.expr.clone()).map(|e| (true, e)),
        )))
        .then_ignore(punct(Punctuation::RightParenthesis))
        .then(stmt.clone())
        .map(|(((is_await, left), (is_of, right)), body)| {
            if is_of {
                Statement::ForOf {
                    is_await: is_await.is_some(),
                    left,
                    right,
                    body,
                }
            } else {
                Statement::ForIn { left, right, body }
            }
        })
        .boxed();

    let for_init = choice((
        decl.clone().map(ForInit::VarDecl),
        seq.clone().map(ForInit::Expr),
    ));
    let for_c = FOR::parser(())
        .ignore_then(punct(Punctuation::LeftParenthesis))
        .ignore_then(for_init.or_not())
        .then_ignore(semicolon.clone())
        .then(seq.clone().or_not())
        .then_ignore(semicolon.clone())
        .then(seq.clone().or_not())
        .then_ignore(punct(Punctuation::RightParenthesis))
        .then(stmt.clone())
        .map(|(((init, test), update), body)| Statement::For {
            init,
            test,
            update,
            body,
        })
        .boxed();

    let while_stmt = WHILE::parser(())
        .ignore_then(paren_seq.clone())
        .then(stmt.clone())
        .map(|(test, body)| Statement::While { test, body })
        .boxed();

    // The `;` after `do ... while (x)` may be omitted even on the same line.
    let do_while = DO::parser(())
        .ignore_then(stmt.clone())
        .then_ignore(WHILE::parser(()))
        .then(paren_seq.clone())
        .then_ignore(semicolon.clone().or_not())
        .map(|(body, test)| Statement::DoWhile { body, test })
        .boxed();

    let return_stmt = RETURN::parser(())
        .then(same_line().ignore_then(seq.clone()).or_not())
        .then_ignore(semi())
        .map(|(kw, value)| Statement::Return {
            span: kw.span,
            value,
        })
        .boxed();
    let break_stmt = BREAK::parser(())
        .then(label.clone())
        .then_ignore(semi())
        .map(|(kw, label)| Statement::Break {
            span: kw.span,
            label,
        })
        .boxed();
    let continue_stmt = CONTINUE::parser(())
        .then(label)
        .then_ignore(semi())
        .map(|(kw, label)| Statement::Continue {
            span: kw.span,
            label,
        })
        .boxed();
    let throw_stmt = THROW::parser(())
        .ignore_then(same_line().labelled(String::from("expression on the same line as `throw`")))
        .ignore_then(seq.clone())
        .then_ignore(semi())
        .map(Statement::Throw)
        .boxed();

    let catch = CATCH::parser(())
        .ignore_then(
            ctx.pattern
                .clone()
                .delimited_by(
                    punct(Punctuation::LeftParenthesis),
                    punct(Punctuation::RightParenthesis),
                )
                .or_not(),
        )
        .then(block.clone())
        .map(|(param, body)| Catch { param, body });
    let finally = FINALLY::parser(()).ignore_then(block.clone());
    let try_stmt = TRY::parser(())
        .ignore_then(block.clone())
        .then(choice((
            catch
                .then(finally.clone().or_not())
                .map(|(c, f)| (Some(c), f)),
            finally.map(|f| (None, Some(f))),
        )))
        .map(|(block, (handler, finalizer))| Statement::Try {
            block,
            handler,
            finalizer,
        })
        .boxed();

    let case = choice((
        CASE::parser(())
            .then(seq.clone())
            .map(|(kw, test)| (kw.span, Some(test))),
        DEFAULT::parser(()).map(|kw| (kw.span, None)),
    ))
    .then_ignore(punct(Punctuation::Colon))
    .then(ctx.stmt.clone().repeated().collect::<Vec<_>>())
    .map(|((span, test), body)| SwitchCase { span, test, body });
    let switch_stmt = SWITCH::parser(())
        .ignore_then(paren_seq)
        .then(
            case.repeated()
                .collect::<Vec<_>>()
                .delimited_by(punct(Punctuation::LeftBrace), punct(Punctuation::RightBrace)),
        )
        .map(|(discriminant, cases)| Statement::Switch {
            discriminant,
            cases,
        })
        .boxed();

    let labeled = Ident::parser(())
        .then_ignore(punct(Punctuation::Colon))
        .then(stmt)
        .map(|(label, body)| Statement::Labeled { label, body })
        .boxed();
    let debugger = DEBUGGER::parser(())
        .then_ignore(semi())
        .to(Statement::Debugger)
        .boxed();
    let expr_stmt = seq.then_ignore(semi()).map(Statement::Expr).boxed();

    // Order matters where prefixes overlap: a leading `{` is a block (never an
    // object literal), `function`/`class` are declarations, and a label is
    // tried before the identifier is taken as an expression.
    choice((
        block.map(Statement::Block).boxed(),
        semicolon.to(Statement::Empty).boxed(),
        decl.then_ignore(semi()).map(Statement::VarDecl).boxed(),
        function_parser(ctx.clone(), true)
            .map(|f| Statement::Function(Box::new(f)))
            .boxed(),
        class_parser(ctx.clone(), true)
            .map(|c| Statement::Class(Box::new(c)))
            .boxed(),
        if_stmt,
        for_in_of,
        for_c,
        while_stmt,
        do_while,
        return_stmt,
        break_stmt,
        continue_stmt,
        throw_stmt,
        try_stmt,
        switch_stmt,
        debugger,
        import_parser(),
        export_parser(ctx),
        labeled,
        expr_stmt,
    ))
    .boxed()
}

/// `(block s...)`
pub(crate) struct BlockDisplay<'a>(pub &'a [Statement]);

impl fmt::Display for BlockDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(block{})", Spaced(self.0))
    }
}

impl fmt::Display for Statement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Statement::Empty => f.write_str("(empty)"),
            Statement::Block(body) => write!(f, "{}", BlockDisplay(body)),
            Statement::Expr(e) => write!(f, "{e}"),
            Statement::VarDecl(d) => write!(f, "{d}"),
            Statement::Function(func) => write!(f, "{func}"),
            Statement::Class(class) => write!(f, "{class}"),
            Statement::If {
                test,
                then,
                otherwise,
            } => {
                write!(f, "(if {test} {then}")?;
                if let Some(o) = otherwise {
                    write!(f, " {o}")?;
                }
                f.write_str(")")
            }
            Statement::For {
                init,
                test,
                update,
                body,
            } => write!(
                f,
                "(for {} {} {} {body})",
                OrBlank(init),
                OrBlank(test),
                OrBlank(update)
            ),
            Statement::ForIn { left, right, body } => write!(f, "(for-in {left} {right} {body})"),
            Statement::ForOf {
                is_await,
                left,
                right,
                body,
            } => {
                let head = if *is_await { "for-await-of" } else { "for-of" };
                write!(f, "({head} {left} {right} {body})")
            }
            Statement::While { test, body } => write!(f, "(while {test} {body})"),
            Statement::DoWhile { body, test } => write!(f, "(do-while {body} {test})"),
            Statement::Return { value: None, .. } => f.write_str("(return)"),
            Statement::Return { value: Some(v), .. } => write!(f, "(return {v})"),
            Statement::Break { label: None, .. } => f.write_str("(break)"),
            Statement::Break { label: Some(l), .. } => write!(f, "(break {l})"),
            Statement::Continue { label: None, .. } => f.write_str("(continue)"),
            Statement::Continue { label: Some(l), .. } => write!(f, "(continue {l})"),
            Statement::Throw(e) => write!(f, "(throw {e})"),
            Statement::Try {
                block,
                handler,
                finalizer,
            } => {
                write!(f, "(try {}", BlockDisplay(block))?;
                if let Some(c) = handler {
                    write!(f, " {c}")?;
                }
                if let Some(fin) = finalizer {
                    write!(f, " (finally {})", BlockDisplay(fin))?;
                }
                f.write_str(")")
            }
            Statement::Switch {
                discriminant,
                cases,
            } => write!(f, "(switch {discriminant}{})", Spaced(cases)),
            Statement::Labeled { label, body } => write!(f, "(label {label} {body})"),
            Statement::Debugger => f.write_str("(debugger)"),
            Statement::Import(d) => write!(f, "{d}"),
            Statement::Export { decl, .. } => write!(f, "{decl}"),
        }
    }
}

impl fmt::Display for DeclKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DeclKind::Let(_) => "let",
            DeclKind::Const(_) => "const",
            DeclKind::Var(_) => "var",
        })
    }
}

impl fmt::Display for VarDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}{})", self.kind, Spaced(&self.decls))
    }
}

impl fmt::Display for Declarator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.init {
            Some(init) => write!(f, "(= {} {init})", self.target),
            None => write!(f, "{}", self.target),
        }
    }
}

impl fmt::Display for ForInit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ForInit::VarDecl(d) => write!(f, "{d}"),
            ForInit::Expr(e) => write!(f, "{e}"),
        }
    }
}

impl fmt::Display for ForLeft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ForLeft::Decl(kind, target) => write!(f, "({kind} {target})"),
            ForLeft::Expr(e) => write!(f, "{e}"),
        }
    }
}

impl fmt::Display for Catch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("(catch")?;
        if let Some(p) = &self.param {
            write!(f, " {p}")?;
        }
        write!(f, " {})", BlockDisplay(&self.body))
    }
}

impl fmt::Display for SwitchCase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.test {
            Some(t) => write!(f, "(case {t}{})", Spaced(&self.body)),
            None => write!(f, "(default{})", Spaced(&self.body)),
        }
    }
}

impl fmt::Display for ModuleSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.alias {
            Some(alias) => write!(f, "(as {} {alias})", self.name),
            None => write!(f, "{}", self.name),
        }
    }
}

impl fmt::Display for ImportDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(import {:?}", self.source)?;
        if let Some(d) = &self.default {
            write!(f, " (default {d})")?;
        }
        if let Some(ns) = &self.namespace {
            write!(f, " (* {ns})")?;
        }
        if !self.named.is_empty() {
            write!(f, " (named{})", Spaced(&self.named))?;
        }
        f.write_str(")")
    }
}

impl fmt::Display for ExportDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExportDecl::Decl(stmt) => write!(f, "(export {stmt})"),
            ExportDecl::Default(e) => write!(f, "(export-default {e})"),
            ExportDecl::Named { specs, source } => {
                write!(f, "(export (named{})", Spaced(specs))?;
                if let Some(s) = source {
                    write!(f, " {s:?}")?;
                }
                f.write_str(")")
            }
            ExportDecl::All { alias, source } => match alias {
                Some(ns) => write!(f, "(export (* {ns}) {source:?})"),
                None => write!(f, "(export * {source:?})"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use chumsky::{IterParser, Parser, error::Rich, extra, prelude::end};

    use crate::{lexer::tokenize, parser::JSCtx, token::TokenStruct};

    type TokInput<'src> = &'src [TokenStruct<'src>];
    type TokExtra<'src> = extra::Err<Rich<'src, TokenStruct<'src>>>;

    /// Parse a whole program; one S-expression per top-level statement.
    fn sexpr(src: &str) -> String {
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("lex {src:?}: {e:?}"));
        JSCtx::<TokInput, TokExtra>::build()
            .stmt
            .repeated()
            .collect::<Vec<_>>()
            .then_ignore(end())
            .parse(&tokens[..])
            .into_result()
            .unwrap_or_else(|e| panic!("parse {src:?}: {e:?}"))
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn rejects(src: &str) -> bool {
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("lex {src:?}: {e:?}"));
        JSCtx::<TokInput, TokExtra>::build()
            .stmt
            .repeated()
            .collect::<Vec<_>>()
            .then_ignore(end())
            .parse(&tokens[..])
            .has_errors()
    }

    #[test]
    fn declarations() {
        assert_eq!(sexpr("let a = 1 + 2;"), "(let (= a (+ 1 2)))");
        assert_eq!(sexpr("var a, b = 2"), "(var a (= b 2))");
        assert_eq!(
            sexpr("const { x, y: [z = 1, ...rest] } = obj"),
            "(const (= {x y:[z=1 ...rest]} obj))"
        );
    }

    #[test]
    fn automatic_semicolons() {
        assert_eq!(sexpr("a = 1\nb = 2"), "(= a 1)\n(= b 2)");
        assert_eq!(sexpr("{ a() }"), "(block (call a))");
        assert_eq!(sexpr("return\nx"), "(return)\nx");
        assert_eq!(sexpr("a\n++b"), "a\n(pre++ b)");
        assert!(rejects("a = 1 b = 2"));
    }

    #[test]
    fn blocks_and_empty() {
        assert_eq!(sexpr("{}"), "(block)");
        assert_eq!(sexpr(";"), "(empty)");
        assert_eq!(
            sexpr("{ let a = 1; let b = a; }"),
            "(block (let (= a 1)) (let (= b a)))"
        );
    }

    #[test]
    fn if_else() {
        assert_eq!(
            sexpr("if (x) { a() } else if (y) b(); else {}"),
            "(if x (block (call a)) (if y (call b) (block)))"
        );
    }

    #[test]
    fn loops() {
        assert_eq!(
            sexpr("for (let i = 0; i < 10; i++) {}"),
            "(for (let (= i 0)) (< i 10) (post++ i) (block))"
        );
        assert_eq!(sexpr("for (;;) break"), "(for _ _ _ (break))");
        assert_eq!(
            sexpr("for (const k in obj) continue"),
            "(for-in (const k) obj (continue))"
        );
        assert_eq!(
            sexpr("for (const [k, v] of entries) f(k)"),
            "(for-of (const [k v]) entries (call f k))"
        );
        assert_eq!(sexpr("for (x of xs);"), "(for-of x xs (empty))");
        assert_eq!(
            sexpr("for await (const x of stream) {}"),
            "(for-await-of (const x) stream (block))"
        );
        assert_eq!(sexpr("while (x) x--"), "(while x (post-- x))");
        assert_eq!(sexpr("do x++; while (x < 3)"), "(do-while (post++ x) (< x 3))");
        assert_eq!(
            sexpr("outer: for (;;) { break outer }"),
            "(label outer (for _ _ _ (block (break outer))))"
        );
    }

    #[test]
    fn function() {
        assert_eq!(
            sexpr("function f(a,b) { if (a<b) {return a+b} else {return a-b}} "),
            "(function f (a b) (block (if (< a b) (block (return (+ a b))) (block (return (- a b))))))"
        );
        assert_eq!(
            sexpr("async function* g() {}"),
            "(async function* g () (block))"
        );
    }

    #[test]
    fn class() {
        assert_eq!(
            sexpr(
                "class A extends B {
                    constructor(x) { super(x) }
                    static create() { return new A() }
                    get v() { return this.x }
                    x = 1
                    static y;
                }"
            ),
            "(class A (extends B) (method constructor (x) (block (call super x))) \
             (static (method create () (block (return (new A))))) \
             (get v () (block (return (. this x)))) (field x 1) (static (field y)))"
        );
    }

    #[test]
    fn try_switch_throw() {
        assert_eq!(
            sexpr("try { f() } catch (e) { throw e } finally { done() }"),
            "(try (block (call f)) (catch e (block (throw e))) (finally (block (call done))))"
        );
        assert_eq!(sexpr("try {} catch {}"), "(try (block) (catch (block)))");
        assert!(rejects("try {}"));
        assert_eq!(
            sexpr("switch (x) { case 1: case 2: a(); break; default: b() }"),
            "(switch x (case 1) (case 2 (call a) (break)) (default (call b)))"
        );
        assert_eq!(sexpr("debugger"), "(debugger)");
    }

    #[test]
    fn modules() {
        assert_eq!(sexpr("import 'side-effect'"), "(import \"side-effect\")");
        assert_eq!(
            sexpr("import React, { useState as useS, useEffect } from 'react';"),
            "(import \"react\" (default React) (named (as useState useS) useEffect))"
        );
        assert_eq!(
            sexpr("import * as path from \"path\""),
            "(import \"path\" (* path))"
        );
        assert_eq!(
            sexpr("export const x = 1"),
            "(export (const (= x 1)))"
        );
        assert_eq!(
            sexpr("export default function () {}"),
            "(export-default (function () (block)))"
        );
        assert_eq!(sexpr("export default a + b;"), "(export-default (+ a b))");
        assert_eq!(
            sexpr("export { a, b as c } from './m'"),
            "(export (named a (as b c)) \"./m\")"
        );
        assert_eq!(sexpr("export * from './m'"), "(export * \"./m\")");
    }

    #[test]
    fn object_literal_needs_parens_at_statement_start() {
        assert_eq!(sexpr("{ a: 1 }"), "(block (label a 1))");
    }
}

