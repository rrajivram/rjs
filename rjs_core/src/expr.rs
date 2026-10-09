use std::fmt;

use chumsky::{
    Boxed, IterParser, Parser,
    extra::ParserExtra,
    label::LabelError,
    prelude::{choice, recursive},
};

use crate::{
    function::{Arrow, Class, Function, Method, arrow_parser, class_parser, function_parser, method_parser},
    ident::Ident,
    keyword::{AWAIT, DELETE, IN, INSTANCEOF, NEW, SUPER, THIS, TYPEOF, VOID, YIELD},
    literal::{Literal, NumberLiteral, StringLiteral},
    parser::{JSCtx, JSParser, TokenInput, oper, punct, same_line},
    pattern::drop_trailing_hole,
    span::TokenSpan,
    token::{Operator, Punctuation},
    utils::Spaced,
};

#[derive(Debug, Clone)]
pub enum Expr {
    Literal(Literal),
    Ident(Ident),
    This(TokenSpan),
    Super(TokenSpan),
    /// `[a, , ...b]` — `None` is a hole.
    Array(Vec<Option<Expr>>),
    Object(Vec<Property>),
    Function(Box<Function>),
    Arrow(Box<Arrow>),
    Class(Box<Class>),
    Paren(Box<Expr>),
    /// `...value` in array literals and call arguments.
    Spread(Box<Expr>),
    Unary {
        op: UnaryOp,
        value: Box<Expr>,
    },
    Update {
        op: UpdateOp,
        prefix: bool,
        target: Box<Expr>,
    },
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
    Conditional {
        test: Box<Expr>,
        then: Box<Expr>,
        otherwise: Box<Expr>,
    },
    Assign {
        target: Box<Expr>,
        op: AssignOp,
        value: Box<Expr>,
    },
    /// `a, b, c`
    Sequence(Vec<Expr>),
    Member {
        object: Box<Expr>,
        property: Ident,
        optional: bool,
    },
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
        optional: bool,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        optional: bool,
    },
    New {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Yield {
        span: TokenSpan,
        delegate: bool,
        value: Option<Box<Expr>>,
    },
}

#[derive(Debug, Clone)]
pub enum Property {
    KeyValue(PropKey, Expr),
    Shorthand(Ident),
    Method(Box<Method>),
    Spread(Expr),
}

#[derive(Debug, Clone)]
pub enum PropKey {
    Ident(Ident),
    /// A string or number key.
    Literal(Literal),
    Computed(Box<Expr>),
}

#[derive(Debug, Clone, Copy)]
pub enum UnaryOp {
    Plus,
    Minus,
    Not,
    BitNot,
    TypeOf,
    Void,
    Delete,
    Await,
}

#[derive(Debug, Clone, Copy)]
pub enum UpdateOp {
    Increment,
    Decrement,
}

#[derive(Debug, Clone, Copy)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
    Shl,
    Shr,
    UShr,
    Lt,
    Gt,
    LtEq,
    GtEq,
    In,
    InstanceOf,
    Eq,
    NotEq,
    StrictEq,
    StrictNotEq,
    BitAnd,
    BitOr,
    BitXor,
    And,
    Or,
    Nullish,
}

#[derive(Debug, Clone, Copy)]
pub enum AssignOp {
    Assign,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
    Shl,
    Shr,
    UShr,
    BitAnd,
    BitOr,
    BitXor,
    And,
    Or,
    Nullish,
}

fn binary(left: Expr, op: BinaryOp, right: Expr) -> Expr {
    Expr::Binary {
        left: Box::new(left),
        op,
        right: Box::new(right),
    }
}

impl<'src, I, E> JSParser<'src, I, E, JSCtx<'src, I, E>> for Expr
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    fn parser(args: JSCtx<'src, I, E>) -> impl Parser<'src, I, Self, E> + Clone {
        args.expr
    }
}

impl<'src, I, E> JSParser<'src, I, E> for Expr
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    fn parser(_args: ()) -> impl Parser<'src, I, Self, E> + Clone {
        JSCtx::build().expr
    }
}

/// An Expression: one or more assignment expressions separated by commas.
pub(crate) fn seq_expr<'src, I, E>(ctx: JSCtx<'src, I, E>) -> impl Parser<'src, I, Expr, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    ctx.expr
        .clone()
        .separated_by(punct(Punctuation::Comma))
        .at_least(1)
        .collect::<Vec<_>>()
        .map(|mut es| {
            if es.len() == 1 {
                es.pop().unwrap()
            } else {
                Expr::Sequence(es)
            }
        })
        .boxed()
}

/// An object/class key: `name`, `'str'`, `1`, or `[computed]`.
pub(crate) fn prop_key<'src, I, E>(ctx: JSCtx<'src, I, E>) -> impl Parser<'src, I, PropKey, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    choice((
        Ident::name_parser().map(PropKey::Ident),
        StringLiteral::parser(()).map(|s| PropKey::Literal(Literal::String(s))),
        NumberLiteral::parser(()).map(|n| PropKey::Literal(Literal::Number(n))),
        ctx.expr
            .clone()
            .delimited_by(
                punct(Punctuation::LeftBracket),
                punct(Punctuation::RightBracket),
            )
            .map(|e| PropKey::Computed(Box::new(e))),
    ))
    .boxed()
}

fn spread_or_expr<'src, I, E>(ctx: JSCtx<'src, I, E>) -> impl Parser<'src, I, Expr, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    choice((
        oper(Operator::Ellipsis)
            .ignore_then(ctx.expr.clone())
            .map(|e| Expr::Spread(Box::new(e))),
        ctx.expr,
    ))
}

/// `(a, ...b,)`
fn arguments<'src, I, E>(ctx: JSCtx<'src, I, E>) -> impl Parser<'src, I, Vec<Expr>, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    spread_or_expr(ctx)
        .separated_by(punct(Punctuation::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(
            punct(Punctuation::LeftParenthesis),
            punct(Punctuation::RightParenthesis),
        )
        .boxed()
}

fn primary<'src, I, E>(ctx: JSCtx<'src, I, E>) -> impl Parser<'src, I, Expr, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    let comma = punct(Punctuation::Comma);
    let array = spread_or_expr(ctx.clone())
        .or_not()
        .separated_by(comma.clone())
        .collect::<Vec<_>>()
        .delimited_by(
            punct(Punctuation::LeftBracket),
            punct(Punctuation::RightBracket),
        )
        .map(|elems| Expr::Array(drop_trailing_hole(elems)));
    let property = choice((
        oper(Operator::Ellipsis)
            .ignore_then(ctx.expr.clone())
            .map(Property::Spread),
        method_parser(ctx.clone()).map(|m| Property::Method(Box::new(m))),
        prop_key(ctx.clone())
            .then_ignore(punct(Punctuation::Colon))
            .then(ctx.expr.clone())
            .map(|(key, value)| Property::KeyValue(key, value)),
        Ident::parser(()).map(Property::Shorthand),
    ));
    let object = property
        .separated_by(comma)
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(punct(Punctuation::LeftBrace), punct(Punctuation::RightBrace))
        .map(Expr::Object);
    let paren = seq_expr(ctx.clone())
        .delimited_by(
            punct(Punctuation::LeftParenthesis),
            punct(Punctuation::RightParenthesis),
        )
        .map(|e| Expr::Paren(Box::new(e)));

    choice((
        THIS::parser(()).map(|t| Expr::This(t.span)),
        SUPER::parser(()).map(|t| Expr::Super(t.span)),
        Literal::parser(ctx.clone()).map(Expr::Literal),
        array,
        object,
        function_parser(ctx.clone(), false).map(|f| Expr::Function(Box::new(f))),
        class_parser(ctx, false).map(|c| Expr::Class(Box::new(c))),
        Ident::parser(()).map(Expr::Ident),
        paren,
    ))
    .boxed()
}

#[derive(Clone)]
enum Suffix {
    Member(Ident, bool),
    Index(Expr, bool),
    Call(Vec<Expr>, bool),
}

fn apply_suffix(object: Expr, suffix: Suffix) -> Expr {
    let object = Box::new(object);
    match suffix {
        Suffix::Member(property, optional) => Expr::Member {
            object,
            property,
            optional,
        },
        Suffix::Index(index, optional) => Expr::Index {
            object,
            index: Box::new(index),
            optional,
        },
        Suffix::Call(args, optional) => Expr::Call {
            callee: object,
            args,
            optional,
        },
    }
}

/// LeftHandSideExpression: a primary followed by `.name`, `[index]`, call
/// arguments and optional chains, or a `new` expression.
pub(crate) fn lhs_body<'src, I, E>(ctx: JSCtx<'src, I, E>) -> impl Parser<'src, I, Expr, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    let args = arguments(ctx.clone());
    let dot = punct(Punctuation::Period).ignore_then(Ident::name_parser());
    let index = seq_expr(ctx.clone()).delimited_by(
        punct(Punctuation::LeftBracket),
        punct(Punctuation::RightBracket),
    );
    let primary = primary(ctx);

    // `new` binds to a member chain without calls: in `new a.b(c).d`, `(c)`
    // are the constructor arguments and `.d` applies to the new object.
    let member = recursive(|member| {
        let new_expr = NEW::parser(())
            .ignore_then(member)
            .then(args.clone().or_not())
            .map(|(callee, args)| Expr::New {
                callee: Box::new(callee),
                args: args.unwrap_or_default(),
            });
        choice((new_expr, primary)).foldl(
            choice((
                dot.clone().map(|n| Suffix::Member(n, false)),
                index.clone().map(|i| Suffix::Index(i, false)),
            ))
            .repeated(),
            apply_suffix,
        )
    });
    let optional = oper(Operator::OptionalChain).ignore_then(choice((
        args.clone().map(|a| Suffix::Call(a, true)),
        index.clone().map(|i| Suffix::Index(i, true)),
        Ident::name_parser().map(|n| Suffix::Member(n, true)),
    )));
    member
        .foldl(
            choice((
                args.map(|a| Suffix::Call(a, false)),
                dot.map(|n| Suffix::Member(n, false)),
                index.map(|i| Suffix::Index(i, false)),
                optional,
            ))
            .repeated(),
            apply_suffix,
        )
        .boxed()
}

/// A choice over operator tokens, each mapped to its AST operator.
fn ops<'src, I, E, T>(table: &[(Operator, T)]) -> impl Parser<'src, I, T, E> + Clone + use<'src, I, E, T>
where
    I: TokenInput<'src>,
    E: ParserExtra<'src, I>,
    E::Error: LabelError<'src, I, String>,
    T: Clone,
{
    choice(
        table
            .iter()
            .map(|(o, t)| oper(*o).to(t.clone()))
            .collect::<Vec<_>>(),
    )
}

/// One left-associative binary precedence level.
fn left_assoc<'src, I, E, P, O>(operand: P, op: O) -> Boxed<'src, 'src, I, Expr, E>
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    P: Parser<'src, I, Expr, E> + Clone + 'src,
    O: Parser<'src, I, BinaryOp, E> + Clone + 'src,
{
    operand
        .clone()
        .foldl(op.then(operand).repeated(), |l, (op, r)| binary(l, op, r))
        .boxed()
}

/// AssignmentExpression, the root of the expression grammar.
pub(crate) fn expr_body<'src, I, E>(ctx: JSCtx<'src, I, E>) -> impl Parser<'src, I, Expr, E> + Clone
where
    I: TokenInput<'src> + 'src,
    E: ParserExtra<'src, I> + 'src,
    E::Error: LabelError<'src, I, String>,
{
    use BinaryOp as B;
    use Operator as O;

    let expr = ctx.expr.clone();
    let update_op = choice((
        oper(O::Increment).to(UpdateOp::Increment),
        oper(O::Decrement).to(UpdateOp::Decrement),
    ));
    let postfix = ctx
        .lhs
        .clone()
        .then(same_line().ignore_then(update_op.clone()).or_not())
        .map(|(target, op)| match op {
            Some(op) => Expr::Update {
                op,
                prefix: false,
                target: Box::new(target),
            },
            None => target,
        });
    let unary_op = choice((
        oper(O::Plus).to(UnaryOp::Plus),
        oper(O::Minus).to(UnaryOp::Minus),
        oper(O::LogicalNot).to(UnaryOp::Not),
        oper(O::BitNot).to(UnaryOp::BitNot),
        TYPEOF::parser(()).to(UnaryOp::TypeOf),
        VOID::parser(()).to(UnaryOp::Void),
        DELETE::parser(()).to(UnaryOp::Delete),
        AWAIT::parser(()).to(UnaryOp::Await),
    ));
    // `**` is right-associative, and its right operand may itself be unary
    // (`2 ** -1`).
    let unary = recursive(|unary| {
        let exponent = postfix
            .then(oper(O::Exp).ignore_then(unary.clone()).or_not())
            .map(|(base, exp)| match exp {
                Some(exp) => binary(base, B::Exp, exp),
                None => base,
            });
        choice((
            unary_op
                .then(unary.clone())
                .map(|(op, value)| Expr::Unary {
                    op,
                    value: Box::new(value),
                }),
            update_op.then(unary).map(|(op, target)| Expr::Update {
                op,
                prefix: true,
                target: Box::new(target),
            }),
            exponent,
        ))
    })
    .boxed();

    let mul = left_assoc(
        unary,
        ops(&[(O::Multiply, B::Mul), (O::Divide, B::Div), (O::Modulo, B::Mod)]),
    );
    let add = left_assoc(mul, ops(&[(O::Plus, B::Add), (O::Minus, B::Sub)]));
    let shift = left_assoc(
        add,
        ops(&[(O::Shl, B::Shl), (O::Shr, B::Shr), (O::UShr, B::UShr)]),
    );
    let relational = left_assoc(
        shift,
        choice((
            ops(&[
                (O::Lt, B::Lt),
                (O::Gt, B::Gt),
                (O::LtEq, B::LtEq),
                (O::GtEq, B::GtEq),
            ]),
            INSTANCEOF::parser(()).to(B::InstanceOf),
            IN::parser(()).to(B::In),
        )),
    );
    let equality = left_assoc(
        relational,
        ops(&[
            (O::Eq, B::Eq),
            (O::NotEq, B::NotEq),
            (O::FullEq, B::StrictEq),
            (O::AnyNotEq, B::StrictNotEq),
        ]),
    );
    let bit_and = left_assoc(equality, ops(&[(O::BitAnd, B::BitAnd)]));
    let bit_xor = left_assoc(bit_and, ops(&[(O::BitXor, B::BitXor)]));
    let bit_or = left_assoc(bit_xor, ops(&[(O::BitOr, B::BitOr)]));
    let and = left_assoc(bit_or, ops(&[(O::LogicalAnd, B::And)]));
    let or = left_assoc(and, ops(&[(O::LogicalOr, B::Or), (O::Nullish, B::Nullish)]));

    let conditional = or
        .then(
            punct(Punctuation::QuestionMark)
                .ignore_then(expr.clone())
                .then_ignore(punct(Punctuation::Colon))
                .then(expr.clone())
                .or_not(),
        )
        .map(|(test, branches)| match branches {
            Some((then, otherwise)) => Expr::Conditional {
                test: Box::new(test),
                then: Box::new(then),
                otherwise: Box::new(otherwise),
            },
            None => test,
        });

    let assign_op = choice((
        punct(Punctuation::Equals).to(AssignOp::Assign),
        ops(&[
            (O::PlusEq, AssignOp::Add),
            (O::MinusEq, AssignOp::Sub),
            (O::MulEq, AssignOp::Mul),
            (O::DivEq, AssignOp::Div),
            (O::ModuloEq, AssignOp::Mod),
            (O::ExpEq, AssignOp::Exp),
            (O::ShlEq, AssignOp::Shl),
            (O::ShrEq, AssignOp::Shr),
            (O::UShrEq, AssignOp::UShr),
            (O::BitAndEq, AssignOp::BitAnd),
            (O::BitOrEq, AssignOp::BitOr),
            (O::BitXorEq, AssignOp::BitXor),
            (O::LogicalAndEq, AssignOp::And),
            (O::LogicalOrEq, AssignOp::Or),
            (O::NullishEq, AssignOp::Nullish),
        ]),
    ));
    // Parse the left side once as a conditional rather than trying a
    // dedicated target parser first: retrying would re-parse every nested
    // argument list, which is exponential in nesting depth.
    let assignment = conditional
        .then(assign_op.then(expr.clone()).or_not())
        .map(|(target, rhs)| match rhs {
            Some((op, value)) => Expr::Assign {
                target: Box::new(target),
                op,
                value: Box::new(value),
            },
            None => target,
        });

    let yield_expr = YIELD::parser(())
        .then(choice((
            oper(O::Multiply)
                .ignore_then(expr.clone())
                .map(|v| (true, Some(v))),
            same_line()
                .ignore_then(expr.clone())
                .or_not()
                .map(|v| (false, v)),
        )))
        .map(|(kw, (delegate, value))| Expr::Yield {
            span: kw.span,
            delegate,
            value: value.map(Box::new),
        });

    choice((arrow_parser(ctx), yield_expr, assignment)).boxed()
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Literal(lit) => write!(f, "{lit}"),
            Expr::Ident(i) => write!(f, "{i}"),
            Expr::This(_) => f.write_str("this"),
            Expr::Super(_) => f.write_str("super"),
            Expr::Array(elems) => {
                f.write_str("(array")?;
                for e in elems {
                    match e {
                        Some(e) => write!(f, " {e}")?,
                        None => f.write_str(" _")?,
                    }
                }
                f.write_str(")")
            }
            Expr::Object(props) => write!(f, "(object{})", Spaced(props)),
            Expr::Function(func) => write!(f, "{func}"),
            Expr::Arrow(arrow) => write!(f, "{arrow}"),
            Expr::Class(class) => write!(f, "{class}"),
            Expr::Paren(inner) => write!(f, "(paren {inner})"),
            Expr::Spread(inner) => write!(f, "(... {inner})"),
            Expr::Unary { op, value } => write!(f, "({op} {value})"),
            Expr::Update { op, prefix, target } => {
                let fix = if *prefix { "pre" } else { "post" };
                write!(f, "({fix}{op} {target})")
            }
            Expr::Binary { left, op, right } => write!(f, "({op} {left} {right})"),
            Expr::Conditional {
                test,
                then,
                otherwise,
            } => write!(f, "(? {test} {then} {otherwise})"),
            Expr::Assign { target, op, value } => write!(f, "({op} {target} {value})"),
            Expr::Sequence(es) => write!(f, "(,{})", Spaced(es)),
            Expr::Member {
                object,
                property,
                optional,
            } => {
                let op = if *optional { "?." } else { "." };
                write!(f, "({op} {object} {property})")
            }
            Expr::Index {
                object,
                index,
                optional,
            } => {
                let op = if *optional { "?.[]" } else { "[]" };
                write!(f, "({op} {object} {index})")
            }
            Expr::Call {
                callee,
                args,
                optional,
            } => {
                let op = if *optional { "?.call" } else { "call" };
                write!(f, "({op} {callee}{})", Spaced(args))
            }
            Expr::New { callee, args } => write!(f, "(new {callee}{})", Spaced(args)),
            Expr::Yield {
                delegate, value, ..
            } => {
                f.write_str(if *delegate { "(yield*" } else { "(yield" })?;
                if let Some(v) = value {
                    write!(f, " {v}")?;
                }
                f.write_str(")")
            }
        }
    }
}

impl fmt::Display for Property {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Property::KeyValue(key, value) => write!(f, "(: {key} {value})"),
            Property::Shorthand(name) => write!(f, "{name}"),
            Property::Method(method) => write!(f, "{method}"),
            Property::Spread(value) => write!(f, "(... {value})"),
        }
    }
}

impl fmt::Display for PropKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PropKey::Ident(i) => write!(f, "{i}"),
            PropKey::Literal(lit) => write!(f, "{lit}"),
            PropKey::Computed(e) => write!(f, "[{e}]"),
        }
    }
}

impl fmt::Display for UnaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UnaryOp::Plus => "+",
            UnaryOp::Minus => "-",
            UnaryOp::Not => "!",
            UnaryOp::BitNot => "~",
            UnaryOp::TypeOf => "typeof",
            UnaryOp::Void => "void",
            UnaryOp::Delete => "delete",
            UnaryOp::Await => "await",
        })
    }
}

impl fmt::Display for UpdateOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UpdateOp::Increment => "++",
            UpdateOp::Decrement => "--",
        })
    }
}

impl fmt::Display for BinaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Mod => "%",
            BinaryOp::Exp => "**",
            BinaryOp::Shl => "<<",
            BinaryOp::Shr => ">>",
            BinaryOp::UShr => ">>>",
            BinaryOp::Lt => "<",
            BinaryOp::Gt => ">",
            BinaryOp::LtEq => "<=",
            BinaryOp::GtEq => ">=",
            BinaryOp::In => "in",
            BinaryOp::InstanceOf => "instanceof",
            BinaryOp::Eq => "==",
            BinaryOp::NotEq => "!=",
            BinaryOp::StrictEq => "===",
            BinaryOp::StrictNotEq => "!==",
            BinaryOp::BitAnd => "&",
            BinaryOp::BitOr => "|",
            BinaryOp::BitXor => "^",
            BinaryOp::And => "&&",
            BinaryOp::Or => "||",
            BinaryOp::Nullish => "??",
        })
    }
}

impl fmt::Display for AssignOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AssignOp::Assign => "=",
            AssignOp::Add => "+=",
            AssignOp::Sub => "-=",
            AssignOp::Mul => "*=",
            AssignOp::Div => "/=",
            AssignOp::Mod => "%=",
            AssignOp::Exp => "**=",
            AssignOp::Shl => "<<=",
            AssignOp::Shr => ">>=",
            AssignOp::UShr => ">>>=",
            AssignOp::BitAnd => "&=",
            AssignOp::BitOr => "|=",
            AssignOp::BitXor => "^=",
            AssignOp::And => "&&=",
            AssignOp::Or => "||=",
            AssignOp::Nullish => "??=",
        })
    }
}

#[cfg(test)]
mod tests {
    use chumsky::{Parser, error::Rich, extra, prelude::end};

    use super::Expr;
    use crate::{lexer::tokenize, parser::JSParser, token::TokenStruct};

    type TokInput<'src> = &'src [TokenStruct<'src>];
    type TokExtra<'src> = extra::Err<Rich<'src, TokenStruct<'src>>>;

    fn sexpr(src: &str) -> String {
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("lex {src:?}: {e:?}"));
        <Expr as JSParser<TokInput, TokExtra>>::parser(())
            .then_ignore(end())
            .parse(&tokens[..])
            .into_result()
            .unwrap_or_else(|e| panic!("parse {src:?}: {e:?}"))
            .to_string()
    }

    fn rejects(src: &str) -> bool {
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("lex {src:?}: {e:?}"));
        <Expr as JSParser<TokInput, TokExtra>>::parser(())
            .then_ignore(end())
            .parse(&tokens[..])
            .has_errors()
    }

    #[test]
    fn precedence_and_associativity() {
        assert_eq!(sexpr("1 + 2 * 3"), "(+ 1 (* 2 3))");
        assert_eq!(sexpr("1 - 2 - 3"), "(- (- 1 2) 3)");
        assert_eq!(sexpr("-x * !y"), "(* (- x) (! y))");
        assert_eq!(sexpr("(1 + 2) * 3"), "(* (paren (+ 1 2)) 3)");
        assert_eq!(sexpr("a < b == c < d"), "(== (< a b) (< c d))");
        assert_eq!(sexpr("a || b && c | d ^ e & f"), "(|| a (&& b (| c (^ d (& e f)))))");
        assert_eq!(sexpr("a << 1 + 2"), "(<< a (+ 1 2))");
        assert_eq!(sexpr("a ?? b"), "(?? a b)");
        assert_eq!(sexpr("x instanceof Y && 'k' in o"), "(&& (instanceof x Y) (in \"k\" o))");
    }

    #[test]
    fn exponent_is_right_associative() {
        assert_eq!(sexpr("2 ** 3 ** 2"), "(** 2 (** 3 2))");
        assert_eq!(sexpr("2 ** -1"), "(** 2 (- 1))");
    }

    #[test]
    fn unary_and_update() {
        assert_eq!(sexpr("typeof x === 'string'"), "(=== (typeof x) \"string\")");
        assert_eq!(sexpr("delete a.b"), "(delete (. a b))");
        assert_eq!(sexpr("void 0"), "(void 0)");
        assert_eq!(sexpr("~~x"), "(~ (~ x))");
        assert_eq!(sexpr("i++"), "(post++ i)");
        assert_eq!(sexpr("--i"), "(pre-- i)");
        assert_eq!(sexpr("await f()"), "(await (call f))");
    }

    #[test]
    fn assignment() {
        assert_eq!(sexpr("a = b = 1"), "(= a (= b 1))");
        assert_eq!(sexpr("a.f = foo(bar, 2)"), "(= (. a f) (call foo bar 2))");
        assert_eq!(sexpr("x += 1"), "(+= x 1)");
        assert_eq!(sexpr("x ??= y || z"), "(??= x (|| y z))");
        assert_eq!(sexpr("[a, b] = [b, a]"), "(= (array a b) (array b a))");
    }

    #[test]
    fn conditional() {
        assert_eq!(sexpr("a ? b : c ? d : e"), "(? a b (? c d e))");
        assert_eq!(sexpr("a ? b = 1 : c"), "(? a (= b 1) c)");
        assert_eq!(sexpr("a?.5:1"), "(? a .5 1)");
    }

    #[test]
    fn member_call_new() {
        assert_eq!(sexpr("a.b.c"), "(. (. a b) c)");
        assert_eq!(sexpr("a[i + 1]"), "([] a (+ i 1))");
        assert_eq!(sexpr("f(1)(2)"), "(call (call f 1) 2)");
        assert_eq!(sexpr("a.if.class"), "(. (. a if) class)");
        assert_eq!(sexpr("new Foo"), "(new Foo)");
        assert_eq!(sexpr("new Foo(1, 2)"), "(new Foo 1 2)");
        assert_eq!(sexpr("new a.B(c).d"), "(. (new (. a B) c) d)");
        assert_eq!(sexpr("new new X()()"), "(new (new X))");
        assert_eq!(sexpr("f(...args, 1,)"), "(call f (... args) 1)");
        assert_eq!(sexpr("super.m()"), "(call (. super m))");
        assert_eq!(sexpr("this.x"), "(. this x)");
    }

    #[test]
    fn optional_chaining() {
        assert_eq!(sexpr("a?.b"), "(?. a b)");
        assert_eq!(sexpr("a?.[0]"), "(?.[] a 0)");
        assert_eq!(sexpr("a?.(x).y"), "(. (?.call a x) y)");
    }

    #[test]
    fn literals() {
        assert_eq!(sexpr("1.5"), "1.5");
        assert_eq!(sexpr("'hi'"), "\"hi\"");
        assert_eq!(sexpr("\"say \\\"hi\\\"\\n\""), "\"say \\\"hi\\\"\\n\"");
        assert_eq!(sexpr("'\\u0041\\x42\\u{1F600}'"), "\"AB😀\"");
        assert_eq!(sexpr("0xff"), "0xff");
        assert_eq!(sexpr("1e-3"), "1e-3");
        assert_eq!(sexpr("true"), "true");
        assert_eq!(sexpr("null"), "null");
    }

    #[test]
    fn arrays_and_objects() {
        assert_eq!(sexpr("[]"), "(array)");
        assert_eq!(sexpr("[1, , 2,]"), "(array 1 _ 2)");
        assert_eq!(sexpr("[...a, b]"), "(array (... a) b)");
        assert_eq!(sexpr("({})"), "(paren (object))");
        assert_eq!(
            sexpr("({ a: 1, b, 'c': 2, [k]: 3, ...rest, })"),
            "(paren (object (: a 1) b (: \"c\" 2) (: [k] 3) (... rest)))"
        );
        assert_eq!(
            sexpr("({ m(x) { return x }, get v() { return 1 }, set v(x) {}, async *g() {} })"),
            "(paren (object (method m (x) (block (return x))) (get v () (block (return 1))) \
             (set v (x) (block)) (async method* g () (block))))"
        );
        assert_eq!(sexpr("({ get: 1, set })"), "(paren (object (: get 1) set))");
    }

    #[test]
    fn functions_and_arrows() {
        assert_eq!(sexpr("x => x * 2"), "(=> (x) (* x 2))");
        assert_eq!(sexpr("() => {}"), "(=> () (block))");
        assert_eq!(sexpr("(a, b = 1, ...c) => a"), "(=> (a b=1 ...c) a)");
        assert_eq!(sexpr("({ a, b: [c] }) => c"), "(=> ({a b:[c]}) c)");
        assert_eq!(sexpr("async x => await x"), "(async => (x) (await x))");
        assert_eq!(sexpr("() => ({})"), "(=> () (paren (object)))");
        assert_eq!(
            sexpr("function* gen() { yield 1; yield* other() }"),
            "(function* gen () (block (yield 1) (yield* (call other))))"
        );
        assert_eq!(
            sexpr("async function () { await 1 }"),
            "(async function () (block (await 1)))"
        );
        assert_eq!(sexpr("(function () {})()"), "(call (paren (function () (block))))");
        assert_eq!(sexpr("async(1)"), "(call async 1)");
    }

    #[test]
    fn class_expression() {
        assert_eq!(
            sexpr("class extends Base { m() {} }"),
            "(class (extends Base) (method m () (block)))"
        );
    }

    #[test]
    fn sequence_in_parens() {
        assert_eq!(sexpr("(a, b)"), "(paren (, a b))");
    }

    #[test]
    fn rejects_invalid() {
        assert!(rejects("1 +"));
        assert!(rejects("a ? b"));
        assert!(rejects("(a, b"));
    }
}
