//! JavaScript keywords.
//!
//! A single macro invocation generates three things from one list:
//! - the `Keyword` enum (used by the lexer to classify words),
//! - one span-carrying marker struct per keyword (used in AST nodes so a
//!   parsed statement records where each keyword appeared), and
//! - a `JSParser` impl for each marker struct that matches a single
//!   `Token::Word` holding that keyword.
//!
//! NOTE: some keyword structs shadow prelude names (`String`, `Option`,
//! `Some`-adjacent ones like `All`) inside this module — generated code and
//! anything added here must use fully-qualified `::std` / `::core` paths.
#![allow(clippy::upper_case_acronyms, clippy::self_named_constructors)]
macro_rules! define_keywords {
    ($($k:ident = $s:literal),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Keyword {
            $($k,)*
        }

        impl Keyword {
            pub fn get(value: &str) -> ::core::option::Option<Self> {
                match value {
                    $($s => ::core::option::Option::Some(Self::$k),)*
                    _ => ::core::option::Option::None,
                }
            }

            pub fn is_reserved(value: &str) -> bool {
                Self::get(value).is_some_and(Self::reserved)
            }

            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$k => $s,)*
                }
            }
        }

        $(
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub struct $k {
                pub span: crate::span::TokenSpan,
            }

            impl $k {
                pub fn new(span: crate::span::TokenSpan) -> Self {
                    Self { span }
                }
            }

            // Generic over the args type `A`: keywords need no context, so
            // they parse the same whether handed `()` or a `SqlCtx`.
            impl<'src, I, E, A> crate::parser::JSParser<'src, I, E, A> for $k
            where
                I: chumsky::input::Input<'src, Token = crate::token::TokenStruct<'src>>
                    + chumsky::input::ValueInput<'src>
                    + chumsky::input::ExactSizeInput<'src>,
                E: chumsky::extra::ParserExtra<'src, I>,
                E::Error: chumsky::label::LabelError<'src, I, ::std::string::String>,
            {
                fn parser(_args: A) -> impl chumsky::Parser<'src, I, Self, E> + Clone {
                    use chumsky::Parser;
                    chumsky::prelude::any()
                        .try_map(|t: crate::token::TokenStruct<'src>, span| {
                            match t.token {
                                crate::token::Token::Word {
                                    keyword: ::core::option::Option::Some(Keyword::$k),
                                    ..
                                } => ::core::result::Result::Ok($k { span: t.span }),
                                _ => ::core::result::Result::Err(
                                    chumsky::label::LabelError::<'src, I, ::std::string::String>::expected_found(
                                        [::std::string::String::from($s)],
                                        ::core::option::Option::Some(chumsky::util::MaybeRef::Val(t)),
                                        span,
                                    ),
                                ),
                            }
                        })
                }
            }
        )*
    };
}

define_keywords!(
    ABSTRACT = "abstract",
    ARGUMENTS = "arguments",
    AS = "as",
    ASYNC = "async",
    AWAIT = "await",
    BOOLEAN = "boolean",
    BREAK = "break",
    BYTE = "byte",
    CASE = "case",
    CATCH = "catch",
    CHAR = "char",
    CLASS = "class",
    CONST = "const",
    CONTINUE = "continue",
    DEBUGGER = "debugger",
    DEFAULT = "default",
    DELETE = "delete",
    DO = "do",
    DOUBLE = "double",
    ELSE = "else",
    ENUM = "enum",
    EVAL = "eval",
    EXPORT = "export",
    EXTENDS = "extends",
    FALSE = "false",
    FINAL = "final",
    FINALLY = "finally",
    FLOAT = "float",
    FOR = "for",
    FROM = "from",
    GET = "get",
    GOTO = "goto",
    IF = "if",
    IMPLEMENTS = "implements",
    FUNCTION = "function",
    IMPORT = "import",
    IN = "in",
    INSTANCEOF = "instanceof",
    INT = "int",
    INTERFACE = "interface",
    LET = "let",
    LONG = "long",
    NATIVE = "native",
    NEW = "new",
    NULL = "null",
    OF = "of",
    PACKAGE = "package",
    PRIVATE = "private",
    PROTECTED = "protected",
    PUBLIC = "public",
    RETURN = "return",
    SET = "set",
    SHORT = "short",
    STATIC = "static",
    SUPER = "super",
    SWITCH = "switch",
    SYNCHRONIZED = "synchronized",
    THIS = "this",
    THROW = "throw",
    THROWS = "throws",
    TRANSIENT = "transient",
    TRUE = "true",
    TRY = "try",
    TYPEOF = "typeof",
    USING = "using",
    VAR = "var",
    VOID = "void",
    VOLATILE = "volatile",
    WHILE = "while",
    WITH = "with",
    YIELD = "yield",
);

impl Keyword {
    /// Reserved words can never be bare identifiers. The rest are contextual
    /// (`async`, `of`, `get`, ...) or only reserved in old editions/other
    /// languages (`int`, `goto`, ...), so they remain valid names.
    pub fn reserved(self) -> bool {
        !matches!(
            self,
            Self::ABSTRACT
                | Self::ARGUMENTS
                | Self::AS
                | Self::ASYNC
                | Self::BOOLEAN
                | Self::BYTE
                | Self::CHAR
                | Self::DOUBLE
                | Self::EVAL
                | Self::FINAL
                | Self::FLOAT
                | Self::FROM
                | Self::GET
                | Self::GOTO
                | Self::INT
                | Self::LONG
                | Self::NATIVE
                | Self::OF
                | Self::SET
                | Self::SHORT
                | Self::SYNCHRONIZED
                | Self::THROWS
                | Self::TRANSIENT
                | Self::USING
                | Self::VOLATILE
        )
    }
}
