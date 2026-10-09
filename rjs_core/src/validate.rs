//! Static checks over a parsed program, treated as strict-mode module code.
//!
//! - [`Severity::Error`]: code a JS engine rejects before running it (early
//!   errors) — invalid assignment targets, redeclarations, misplaced
//!   `return`/`break`/`await`/`yield`/`super`, ...
//! - [`Severity::Warning`]: code that parses but fails or misbehaves at
//!   runtime — undeclared variables, calling a non-function, argument-count
//!   mismatches, assigning to a constant, ...

use std::{
    collections::{HashMap, HashSet},
    fmt,
};

use crate::{
    expr::{AssignOp, BinaryOp, Expr, PropKey, Property, UnaryOp},
    function::{Arrow, ArrowBody, Class, ClassMember, Function, Method, MethodKind},
    ident::Ident,
    keyword::Keyword,
    literal::Literal,
    pattern::{Pattern, PatternProp},
    span::TokenSpan,
    statement::{DeclKind, ExportDecl, ForInit, ForLeft, Statement, VarDecl},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub span: Option<TokenSpan>,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "{level}: {}", self.message)
    }
}

pub fn validate(program: &[Statement]) -> Vec<Diagnostic> {
    let mut v = Validator::default();
    v.program(program);
    v.diags
}

/// Built-ins and common host globals that are never reported as undeclared.
const GLOBALS: &[&str] = &[
    "AggregateError", "Array", "ArrayBuffer", "Atomics", "BigInt", "BigInt64Array",
    "BigUint64Array", "Boolean", "Buffer", "DataView", "Date", "Error", "EvalError",
    "FinalizationRegistry", "Float32Array", "Float64Array", "Function", "Infinity", "Int16Array",
    "Int32Array", "Int8Array", "Intl", "JSON", "Map", "Math", "NaN", "Number", "Object",
    "Promise", "Proxy", "RangeError", "ReferenceError", "Reflect", "RegExp", "Set",
    "SharedArrayBuffer", "String", "Symbol", "SyntaxError", "TypeError", "URIError",
    "Uint16Array", "Uint32Array", "Uint8Array", "Uint8ClampedArray", "WeakMap", "WeakRef",
    "WeakSet", "__dirname", "__filename", "clearInterval", "clearTimeout", "console",
    "decodeURI", "decodeURIComponent", "document", "encodeURI", "encodeURIComponent", "eval",
    "exports", "fetch", "globalThis", "isFinite", "isNaN", "location", "module", "navigator",
    "parseFloat", "parseInt", "process", "queueMicrotask", "require", "self", "setInterval",
    "setTimeout", "structuredClone", "undefined", "window",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BindKind {
    Var,
    /// A function declaration at function (or module) top level: var-like.
    Function,
    /// A function declaration inside a block: lexical in strict mode.
    BlockFunction,
    Param,
    Let,
    Const,
    Class,
    Import,
    CatchParam,
}

impl BindKind {
    /// Lexical bindings conflict with any other binding of the same name in
    /// the same scope; var-like ones may be redeclared freely.
    fn lexical(self) -> bool {
        !matches!(self, Self::Var | Self::Function | Self::Param)
    }
}

/// The minimum and maximum argument count a function accepts, by the same
/// rule as `Function.prototype.length`: required parameters end at the first
/// default or rest parameter.
#[derive(Debug, Clone, Copy)]
struct Arity {
    required: usize,
    max: Option<usize>,
}

impl Arity {
    fn of(params: &[Pattern]) -> Self {
        let required = params
            .iter()
            .take_while(|p| !matches!(p, Pattern::Default(..) | Pattern::Rest(_)))
            .count();
        let max = (!params.iter().any(|p| matches!(p, Pattern::Rest(_)))).then_some(params.len());
        Arity { required, max }
    }
}

#[derive(Debug, Clone, Copy)]
struct Binding {
    kind: BindKind,
    /// Set for bindings that always hold the same function: an index into
    /// `Validator::funcs`.
    func: Option<usize>,
}

struct FuncInfo {
    arity: Arity,
    /// Reads `arguments`, so it accepts any number of arguments.
    uses_arguments: bool,
}

#[derive(Default)]
struct Scope {
    /// A function (or module) scope, where `var`s land.
    function: bool,
    names: HashMap<String, Binding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FnKind {
    Module,
    Function,
    Arrow,
    Method,
    Constructor { derived: bool },
    ClassField,
}

struct FnCtx {
    kind: FnKind,
    is_async: bool,
    generator: bool,
    /// Enclosing labels, and whether each labels a loop.
    labels: Vec<(String, bool)>,
    loops: usize,
    switches: usize,
    func: Option<usize>,
}

impl FnCtx {
    fn new(kind: FnKind, is_async: bool, generator: bool) -> Self {
        Self {
            kind,
            is_async,
            generator,
            func: None,
            labels: Vec::new(),
            loops: 0,
            switches: 0,
        }
    }
}

#[derive(Default)]
struct Validator {
    diags: Vec<Diagnostic>,
    scopes: Vec<Scope>,
    fns: Vec<FnCtx>,
    exports: HashSet<String>,
    /// `export { name }` must refer to a top-level binding, which may be
    /// declared later in the module.
    export_refs: Vec<Ident>,
    funcs: Vec<FuncInfo>,
    /// AST node address -> `funcs` index, so a function's declaration, its
    /// body and its call sites agree on one `FuncInfo`.
    func_ids: HashMap<usize, usize>,
    /// Argument-count checks, resolved once every function body has been
    /// walked (a body may read `arguments` after the call site is seen).
    pending_calls: Vec<(usize, Ident, usize)>,
}

impl Validator {
    fn error(&mut self, span: Option<TokenSpan>, message: impl Into<String>) {
        self.diags.push(Diagnostic {
            severity: Severity::Error,
            message: message.into(),
            span,
        });
    }

    fn warn(&mut self, span: Option<TokenSpan>, message: impl Into<String>) {
        self.diags.push(Diagnostic {
            severity: Severity::Warning,
            message: message.into(),
            span,
        });
    }

    fn ctx(&mut self) -> &mut FnCtx {
        self.fns.last_mut().expect("validator always has a function context")
    }

    /// The nearest non-arrow function: arrows inherit `super`, `this` and
    /// `arguments` from it.
    fn this_kind(&self) -> FnKind {
        self.fns
            .iter()
            .rev()
            .map(|c| c.kind)
            .find(|k| *k != FnKind::Arrow)
            .unwrap_or(FnKind::Module)
    }

    fn push_scope(&mut self, function: bool) {
        self.scopes.push(Scope {
            function,
            names: HashMap::new(),
        });
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn lookup(&self, name: &str) -> Option<Binding> {
        self.scopes.iter().rev().find_map(|s| s.names.get(name).copied())
    }

    fn func_id<T>(&mut self, node: &T, params: &[Pattern]) -> usize {
        let funcs = &mut self.funcs;
        *self
            .func_ids
            .entry(node as *const T as usize)
            .or_insert_with(|| {
                funcs.push(FuncInfo {
                    arity: Arity::of(params),
                    uses_arguments: false,
                });
                funcs.len() - 1
            })
    }

    /// The `FuncInfo` of a function expression that a `const` is bound to.
    fn init_func(&mut self, init: &Expr) -> Option<usize> {
        match strip_parens(init) {
            Expr::Function(f) => Some(self.func_id(&**f, &f.params)),
            Expr::Arrow(a) => Some(self.func_id(&**a, &a.params)),
            _ => None,
        }
    }

    fn declare(&mut self, id: &Ident, kind: BindKind, func: Option<usize>) {
        if matches!(id.value.as_str(), "eval" | "arguments") {
            self.error(
                Some(id.span),
                format!("Unexpected '{}' as a binding name in strict mode", id.value),
            );
        } else if Keyword::is_reserved(&id.value) {
            self.error(Some(id.span), format!("Unexpected reserved word '{}'", id.value));
        }
        let scope = self.scopes.last_mut().expect("validator always has a scope");
        if let Some(existing) = scope.names.get(&id.value)
            && (existing.kind.lexical() || kind.lexical())
        {
            self.error(
                Some(id.span),
                format!("Identifier '{}' has already been declared", id.value),
            );
            return;
        }
        scope.names.insert(id.value.clone(), Binding { kind, func });
    }

    fn program(&mut self, stmts: &[Statement]) {
        self.fns.push(FnCtx::new(FnKind::Module, true, false));
        self.push_scope(true);
        self.hoist(stmts.iter().collect(), true);
        for s in stmts {
            self.stmt(s, true);
        }
        for id in std::mem::take(&mut self.export_refs) {
            if self.lookup(&id.value).is_none() {
                self.error(Some(id.span), format!("Export '{}' is not defined", id.value));
            }
        }
        self.pop_scope();
        self.fns.pop();
        for (func, callee, n) in std::mem::take(&mut self.pending_calls) {
            let info = &self.funcs[func];
            if !info.uses_arguments {
                self.check_arity(info.arity, &callee, n);
            }
        }
        self.diags
            .sort_by_key(|d| d.span.map_or(usize::MAX, |s| s.start));
    }

    /// Declare everything a scope's statements bind before walking them, so
    /// references to later declarations (hoisting, mutual recursion) resolve.
    fn hoist(&mut self, stmts: Vec<&Statement>, function_scope: bool) {
        let mut vars = Vec::new();
        for s in &stmts {
            collect_vars(s, &mut vars);
        }
        if function_scope {
            for id in &vars {
                self.declare(id, BindKind::Var, None);
            }
        }
        for s in stmts {
            self.hoist_lexical(s, function_scope);
        }
        // `{ let x; { var x } }`: the var hoists past a lexical binding.
        if !function_scope {
            for id in &vars {
                let clash = self.scopes.last().unwrap().names.get(&id.value);
                if clash.is_some_and(|b| b.kind.lexical() && b.kind != BindKind::CatchParam) {
                    self.error(
                        Some(id.span),
                        format!("Identifier '{}' has already been declared", id.value),
                    );
                }
            }
        }
    }

    fn hoist_lexical(&mut self, s: &Statement, function_scope: bool) {
        match s {
            Statement::VarDecl(d) => {
                let kind = match d.kind {
                    DeclKind::Var(_) => return,
                    DeclKind::Let(_) => BindKind::Let,
                    DeclKind::Const(_) => BindKind::Const,
                };
                for decl in &d.decls {
                    let func = match (&decl.target, &decl.init, kind) {
                        (Pattern::Ident(_), Some(init), BindKind::Const) => self.init_func(init),
                        _ => None,
                    };
                    for id in bound_names(&decl.target) {
                        self.declare(&id, kind, func);
                    }
                }
            }
            Statement::Function(f) => {
                if let Some(name) = &f.name {
                    let kind = if function_scope {
                        BindKind::Function
                    } else {
                        BindKind::BlockFunction
                    };
                    let func = self.func_id(&**f, &f.params);
                    self.declare(name, kind, Some(func));
                }
            }
            Statement::Class(c) => {
                if let Some(name) = &c.name {
                    self.declare(name, BindKind::Class, None);
                }
            }
            Statement::Import(decl) => {
                let locals = decl
                    .default
                    .iter()
                    .chain(&decl.namespace)
                    .chain(decl.named.iter().map(|s| s.alias.as_ref().unwrap_or(&s.name)));
                for id in locals.cloned().collect::<Vec<_>>() {
                    self.declare(&id, BindKind::Import, None);
                }
            }
            Statement::Export {
                decl: ExportDecl::Decl(inner),
                ..
            } => self.hoist_lexical(inner, function_scope),
            _ => {}
        }
    }

    fn block(&mut self, body: &[Statement]) {
        self.push_scope(false);
        self.hoist(body.iter().collect(), false);
        for s in body {
            self.stmt(s, false);
        }
        self.pop_scope();
    }

    /// The body of `if`/loops/labels, where only a single statement — not a
    /// declaration — may appear.
    fn sub_stmt(&mut self, s: &Statement) {
        match s {
            Statement::VarDecl(VarDecl { kind, .. }) if !matches!(kind, DeclKind::Var(_)) => {
                let span = match kind {
                    DeclKind::Let(kw) => kw.span,
                    DeclKind::Const(kw) => kw.span,
                    DeclKind::Var(kw) => kw.span,
                };
                self.error(
                    Some(span),
                    "Lexical declaration cannot appear in a single-statement context",
                );
            }
            Statement::Class(c) => self.error(
                c.name.as_ref().map(|n| n.span),
                "Class declaration cannot appear in a single-statement context",
            ),
            Statement::Function(f) => self.error(
                f.name.as_ref().map(|n| n.span),
                "In strict mode code, functions can only be declared at top level or inside a block",
            ),
            _ => {}
        }
        self.stmt(s, false);
    }

    fn loop_body(&mut self, body: &Statement) {
        self.ctx().loops += 1;
        self.sub_stmt(body);
        self.ctx().loops -= 1;
    }

    fn stmt(&mut self, s: &Statement, top_level: bool) {
        match s {
            Statement::Empty | Statement::Debugger => {}
            Statement::Block(body) => self.block(body),
            Statement::Expr(e) => self.expr(e),
            Statement::VarDecl(d) => self.var_decl(d),
            Statement::Function(f) => self.function(f, FnKind::Function),
            Statement::Class(c) => self.class(c),
            Statement::If {
                test,
                then,
                otherwise,
            } => {
                self.expr(test);
                self.sub_stmt(then);
                if let Some(o) = otherwise {
                    self.sub_stmt(o);
                }
            }
            Statement::For {
                init,
                test,
                update,
                body,
            } => {
                self.push_scope(false);
                match init {
                    Some(ForInit::VarDecl(d)) => {
                        self.declare_lexical_head(d);
                        self.var_decl(d);
                    }
                    Some(ForInit::Expr(e)) => self.expr(e),
                    None => {}
                }
                for e in test.iter().chain(update) {
                    self.expr(e);
                }
                self.loop_body(body);
                self.pop_scope();
            }
            Statement::ForIn { left, right, body }
            | Statement::ForOf {
                left, right, body, ..
            } => {
                self.push_scope(false);
                if let Statement::ForOf { is_await: true, .. } = s
                    && !self.ctx().is_async
                {
                    self.error(
                        left_span(left),
                        "for await is only valid in async functions and the top level bodies of modules",
                    );
                }
                self.for_left(left);
                self.expr(right);
                self.loop_body(body);
                self.pop_scope();
            }
            Statement::While { test, body } => {
                self.expr(test);
                self.loop_body(body);
            }
            Statement::DoWhile { body, test } => {
                self.loop_body(body);
                self.expr(test);
            }
            Statement::Return { span, value } => {
                if self.ctx().kind == FnKind::Module {
                    self.error(Some(*span), "Illegal return statement");
                }
                if let Some(v) = value {
                    self.expr(v);
                }
            }
            Statement::Break { span, label } => self.jump(*span, label.as_ref(), false),
            Statement::Continue { span, label } => self.jump(*span, label.as_ref(), true),
            Statement::Throw(e) => self.expr(e),
            Statement::Try {
                block,
                handler,
                finalizer,
            } => {
                self.block(block);
                if let Some(c) = handler {
                    self.push_scope(false);
                    if let Some(p) = &c.param {
                        let mut seen = HashSet::new();
                        for id in bound_names(p) {
                            if !seen.insert(id.value.clone()) {
                                self.error(
                                    Some(id.span),
                                    format!("Identifier '{}' has already been declared", id.value),
                                );
                            } else {
                                self.declare(&id, BindKind::CatchParam, None);
                            }
                        }
                        self.binding_pattern(p);
                    }
                    self.hoist(c.body.iter().collect(), false);
                    for s in &c.body {
                        self.stmt(s, false);
                    }
                    self.pop_scope();
                }
                if let Some(f) = finalizer {
                    self.block(f);
                }
            }
            Statement::Switch {
                discriminant,
                cases,
            } => {
                self.expr(discriminant);
                let mut defaults = cases.iter().filter(|c| c.test.is_none()).skip(1);
                if let Some(dup) = defaults.next() {
                    self.error(Some(dup.span), "More than one default clause in switch statement");
                }
                // All clauses share one block scope.
                self.push_scope(false);
                self.hoist(cases.iter().flat_map(|c| &c.body).collect(), false);
                self.ctx().switches += 1;
                for case in cases {
                    if let Some(t) = &case.test {
                        self.expr(t);
                    }
                    for s in &case.body {
                        self.stmt(s, false);
                    }
                }
                self.ctx().switches -= 1;
                self.pop_scope();
            }
            Statement::Labeled { label, body } => {
                if self.ctx().labels.iter().any(|(l, _)| *l == label.value) {
                    self.error(
                        Some(label.span),
                        format!("Label '{}' has already been declared", label.value),
                    );
                }
                let is_loop = labels_loop(body);
                self.ctx().labels.push((label.value.clone(), is_loop));
                self.sub_stmt(body);
                self.ctx().labels.pop();
            }
            Statement::Import(decl) => {
                if !top_level {
                    self.error(
                        Some(decl.span),
                        "import declarations may only appear at top level of a module",
                    );
                }
            }
            Statement::Export { span, decl } => {
                if !top_level {
                    self.error(
                        Some(*span),
                        "export declarations may only appear at top level of a module",
                    );
                }
                self.export(*span, decl);
            }
        }
    }

    fn add_export(&mut self, name: &str, span: Option<TokenSpan>) {
        if !self.exports.insert(name.to_string()) {
            self.error(span, format!("Duplicate export of '{name}'"));
        }
    }

    fn export(&mut self, span: TokenSpan, decl: &ExportDecl) {
        match decl {
            ExportDecl::Decl(inner) => {
                let names = match &**inner {
                    Statement::VarDecl(d) => d.decls.iter().flat_map(|d| bound_names(&d.target)).collect(),
                    Statement::Function(f) => f.name.iter().cloned().collect(),
                    Statement::Class(c) => c.name.iter().cloned().collect(),
                    _ => Vec::new(),
                };
                for id in names {
                    self.add_export(&id.value, Some(id.span));
                }
                self.stmt(inner, true);
            }
            ExportDecl::Default(e) => {
                self.add_export("default", Some(span));
                self.expr(e);
            }
            ExportDecl::Named { specs, source } => {
                for spec in specs {
                    let exported = spec.alias.as_ref().unwrap_or(&spec.name);
                    self.add_export(&exported.value, Some(exported.span));
                    if source.is_none() {
                        self.export_refs.push(spec.name.clone());
                    }
                }
            }
            ExportDecl::All { alias, .. } => {
                if let Some(a) = alias {
                    self.add_export(&a.value, Some(a.span));
                }
            }
        }
    }

    fn jump(&mut self, span: TokenSpan, label: Option<&Ident>, is_continue: bool) {
        let ctx = self.ctx();
        let message = match label {
            Some(l) => match ctx.labels.iter().rev().find(|(name, _)| *name == l.value) {
                None => Some(format!("Undefined label '{}'", l.value)),
                Some((_, false)) if is_continue => Some(format!(
                    "Illegal continue statement: '{}' does not denote an iteration statement",
                    l.value
                )),
                Some(_) => None,
            },
            None if is_continue && ctx.loops == 0 => {
                Some("Illegal continue statement: no surrounding iteration statement".into())
            }
            None if !is_continue && ctx.loops == 0 && ctx.switches == 0 => {
                Some("Illegal break statement".into())
            }
            None => None,
        };
        if let Some(m) = message {
            self.error(Some(label.map_or(span, |l| l.span)), m);
        }
    }

    /// `let`/`const` in a `for` head bind in the loop's own scope.
    fn declare_lexical_head(&mut self, d: &VarDecl) {
        let kind = match d.kind {
            DeclKind::Var(_) => return,
            DeclKind::Let(_) => BindKind::Let,
            DeclKind::Const(_) => BindKind::Const,
        };
        for id in d.decls.iter().flat_map(|d| bound_names(&d.target)) {
            self.declare(&id, kind, None);
        }
    }

    fn var_decl(&mut self, d: &VarDecl) {
        for decl in &d.decls {
            self.binding_pattern(&decl.target);
            match &decl.init {
                Some(init) => self.expr(init),
                None if matches!(d.kind, DeclKind::Const(_)) => self.error(
                    pattern_span(&decl.target),
                    "Missing initializer in const declaration",
                ),
                None if !matches!(decl.target, Pattern::Ident(_)) => self.error(
                    pattern_span(&decl.target),
                    "Missing initializer in destructuring declaration",
                ),
                None => {}
            }
        }
    }

    fn for_left(&mut self, left: &ForLeft) {
        match left {
            ForLeft::Decl(kind, target) => {
                let kind = match kind {
                    DeclKind::Var(_) => None,
                    DeclKind::Let(_) => Some(BindKind::Let),
                    DeclKind::Const(_) => Some(BindKind::Const),
                };
                if let Some(kind) = kind {
                    for id in bound_names(target) {
                        self.declare(&id, kind, None);
                    }
                }
                self.binding_pattern(target);
            }
            ForLeft::Expr(e) => self.assign_target(e),
        }
    }

    /// Structural checks on a binding pattern, plus its default values.
    fn binding_pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Ident(_) => {}
            Pattern::Array(elems) => {
                for (i, e) in elems.iter().enumerate() {
                    let Some(e) = e else { continue };
                    if matches!(e, Pattern::Rest(_)) && i + 1 != elems.len() {
                        self.error(pattern_span(e), "Rest element must be last element");
                    }
                    self.binding_pattern(e);
                }
            }
            Pattern::Object(props) => {
                for (i, prop) in props.iter().enumerate() {
                    match prop {
                        PatternProp::KeyValue(key, value) => {
                            self.prop_key(key);
                            self.binding_pattern(value);
                        }
                        PatternProp::Shorthand(_, default) => {
                            if let Some(d) = default {
                                self.expr(d);
                            }
                        }
                        PatternProp::Rest(target) => {
                            if i + 1 != props.len() {
                                self.error(pattern_span(target), "Rest element must be last element");
                            }
                            if !matches!(target, Pattern::Ident(_)) {
                                self.error(
                                    pattern_span(target),
                                    "`...` must be followed by an identifier in declaration contexts",
                                );
                            }
                        }
                    }
                }
            }
            Pattern::Default(target, default) => {
                self.binding_pattern(target);
                self.expr(default);
            }
            Pattern::Rest(target) => self.binding_pattern(target),
        }
    }

    fn params(&mut self, params: &[Pattern]) {
        let mut seen = HashSet::new();
        for (i, p) in params.iter().enumerate() {
            if matches!(p, Pattern::Rest(_)) && i + 1 != params.len() {
                self.error(pattern_span(p), "Rest parameter must be last formal parameter");
            }
            for id in bound_names(p) {
                if seen.insert(id.value.clone()) {
                    self.declare(&id, BindKind::Param, None);
                } else {
                    self.error(
                        Some(id.span),
                        format!("Duplicate parameter name '{}' not allowed in this context", id.value),
                    );
                }
            }
            self.binding_pattern(p);
        }
    }

    fn function(&mut self, f: &Function, kind: FnKind) {
        let func = self.func_id(f, &f.params);
        self.fns.push(FnCtx {
            func: Some(func),
            ..FnCtx::new(kind, f.is_async, f.generator)
        });
        self.push_scope(true);
        self.params(&f.params);
        self.hoist(f.body.iter().collect(), true);
        for s in &f.body {
            self.stmt(s, false);
        }
        self.pop_scope();
        self.fns.pop();
    }

    fn arrow(&mut self, a: &Arrow) {
        self.fns.push(FnCtx::new(FnKind::Arrow, a.is_async, false));
        self.push_scope(true);
        self.params(&a.params);
        match &a.body {
            ArrowBody::Expr(e) => self.expr(e),
            ArrowBody::Block(body) => {
                self.hoist(body.iter().collect(), true);
                for s in body {
                    self.stmt(s, false);
                }
            }
        }
        self.pop_scope();
        self.fns.pop();
    }

    fn method(&mut self, m: &Method, kind: FnKind) {
        self.prop_key(&m.key);
        let params = &m.func.params;
        match m.kind {
            MethodKind::Get if !params.is_empty() => {
                self.error(prop_key_span(&m.key), "Getter must not have any formal parameters");
            }
            MethodKind::Set if params.len() != 1 => {
                self.error(prop_key_span(&m.key), "Setter must have exactly one formal parameter");
            }
            MethodKind::Set if matches!(params[0], Pattern::Rest(_)) => {
                self.error(
                    prop_key_span(&m.key),
                    "Setter function argument must not be a rest parameter",
                );
            }
            _ => {}
        }
        self.function(&m.func, kind);
    }

    fn class(&mut self, c: &Class) {
        if let Some(base) = &c.extends {
            self.expr(base);
        }
        // The class name is an immutable binding inside the class body.
        self.push_scope(false);
        if let Some(name) = &c.name {
            self.declare(name, BindKind::Const, None);
        }
        let derived = c.extends.is_some();
        let mut has_constructor = false;
        for member in &c.members {
            match member {
                ClassMember::Method { is_static, method } => {
                    let name = static_key_name(&method.key);
                    let is_constructor = !is_static && name == Some("constructor");
                    if is_constructor {
                        if has_constructor {
                            self.error(
                                prop_key_span(&method.key),
                                "A class may only have one constructor",
                            );
                        }
                        has_constructor = true;
                        if method.kind != MethodKind::Method
                            || method.func.is_async
                            || method.func.generator
                        {
                            self.error(
                                prop_key_span(&method.key),
                                "Class constructor may not be an accessor, generator or async method",
                            );
                        }
                    }
                    if *is_static && name == Some("prototype") {
                        self.error(
                            prop_key_span(&method.key),
                            "Classes may not have a static property named 'prototype'",
                        );
                    }
                    let kind = if is_constructor {
                        FnKind::Constructor { derived }
                    } else {
                        FnKind::Method
                    };
                    self.method(method, kind);
                }
                ClassMember::Field {
                    is_static,
                    key,
                    value,
                } => {
                    let name = static_key_name(key);
                    if name == Some("constructor") {
                        self.error(prop_key_span(key), "Classes may not have a field named 'constructor'");
                    }
                    if *is_static && name == Some("prototype") {
                        self.error(
                            prop_key_span(key),
                            "Classes may not have a static property named 'prototype'",
                        );
                    }
                    self.prop_key(key);
                    if let Some(v) = value {
                        self.fns.push(FnCtx::new(FnKind::ClassField, false, false));
                        self.expr(v);
                        self.fns.pop();
                    }
                }
            }
        }
        self.pop_scope();
    }

    fn prop_key(&mut self, key: &PropKey) {
        if let PropKey::Computed(e) = key {
            self.expr(e);
        }
    }

    fn literal(&mut self, lit: &Literal) {
        if let Literal::Number(n) = lit {
            let mut chars = n.raw.chars();
            if chars.next() == Some('0') && chars.next().is_some_and(|c| c.is_ascii_digit()) {
                self.error(
                    Some(n.span),
                    "Octal literals and decimals with leading zeros are not allowed in strict mode",
                );
            }
        }
    }

    /// A read of a variable.
    fn reference(&mut self, id: &Ident) {
        if self.lookup(&id.value).is_some() || GLOBALS.contains(&id.value.as_str()) {
            return;
        }
        if id.value == "arguments" && !matches!(self.this_kind(), FnKind::Module | FnKind::ClassField) {
            let owner = self.fns.iter().rev().find(|c| c.kind != FnKind::Arrow);
            if let Some(func) = owner.and_then(|c| c.func) {
                self.funcs[func].uses_arguments = true;
            }
            return;
        }
        self.warn(Some(id.span), format!("'{}' is not defined", id.value));
    }

    /// A write to a variable.
    fn write_ref(&mut self, id: &Ident) {
        if matches!(id.value.as_str(), "eval" | "arguments") {
            self.error(
                Some(id.span),
                format!("Unexpected '{}' as an assignment target in strict mode", id.value),
            );
            return;
        }
        match self.lookup(&id.value).map(|b| b.kind) {
            Some(BindKind::Const) => {
                self.warn(Some(id.span), format!("Assignment to constant variable '{}'", id.value));
            }
            Some(BindKind::Import) => {
                self.warn(Some(id.span), format!("Assignment to imported binding '{}'", id.value));
            }
            _ => self.reference(id),
        }
    }

    /// The target of `=`, `for-in` and `for-of`: a simple target or an
    /// array/object destructuring pattern.
    fn assign_target(&mut self, e: &Expr) {
        match e {
            Expr::Array(elems) => {
                for (i, el) in elems.iter().enumerate() {
                    match el {
                        None => {}
                        Some(Expr::Spread(target)) => {
                            if i + 1 != elems.len() {
                                self.error(expr_span(target), "Rest element must be last element");
                            }
                            self.assign_target(target);
                        }
                        Some(el) => self.assign_element(el),
                    }
                }
            }
            Expr::Object(props) => {
                for (i, prop) in props.iter().enumerate() {
                    match prop {
                        Property::KeyValue(key, value) => {
                            self.prop_key(key);
                            self.assign_element(value);
                        }
                        Property::Shorthand(id) => self.write_ref(id),
                        Property::Spread(target) => {
                            if i + 1 != props.len() {
                                self.error(expr_span(target), "Rest element must be last element");
                            }
                            self.simple_target(target, "Invalid rest element in destructuring assignment");
                        }
                        Property::Method(m) => {
                            self.error(prop_key_span(&m.key), "Invalid destructuring assignment target");
                        }
                    }
                }
            }
            _ => self.simple_target(e, "Invalid left-hand side in assignment"),
        }
    }

    /// An element of a destructuring assignment, which may carry a default.
    fn assign_element(&mut self, e: &Expr) {
        match e {
            Expr::Assign {
                target,
                op: AssignOp::Assign,
                value,
            } => {
                self.assign_target(target);
                self.expr(value);
            }
            _ => self.assign_target(e),
        }
    }

    /// A variable or property reference — the only valid target of compound
    /// assignment and `++`/`--`.
    fn simple_target(&mut self, e: &Expr, message: &str) {
        match e {
            Expr::Ident(id) => self.write_ref(id),
            Expr::Member { .. } | Expr::Index { .. } if !is_optional_chain(e) => self.expr(e),
            Expr::Paren(inner)
                if matches!(
                    strip_parens(inner),
                    Expr::Ident(_) | Expr::Member { .. } | Expr::Index { .. }
                ) =>
            {
                self.simple_target(inner, message)
            }
            _ => {
                self.error(expr_span(e), message);
                self.expr(e);
            }
        }
    }

    fn check_callee(&mut self, callee: &Expr, construct: bool) {
        let target = strip_parens(callee);
        let not_function = match target {
            Expr::Literal(lit) => Some(format!("'{lit}' is not a function")),
            Expr::Array(_) => Some("an array literal is not a function".into()),
            Expr::Object(_) => Some("an object literal is not a function".into()),
            Expr::Unary { .. } | Expr::Update { .. } => {
                Some("the result of this operator is not a function".into())
            }
            Expr::Binary { op, .. }
                if !matches!(op, BinaryOp::And | BinaryOp::Or | BinaryOp::Nullish) =>
            {
                Some("the result of this operator is not a function".into())
            }
            _ => None,
        };
        if let Some(why) = not_function {
            let what = if construct { "construct" } else { "call" };
            self.warn(expr_span(callee), format!("Cannot {what} this expression: {why}"));
            return;
        }
        match target {
            Expr::Class(_) if !construct => {
                self.warn(expr_span(callee), "Class constructors cannot be invoked without 'new'");
            }
            Expr::Arrow(_) if construct => {
                self.warn(expr_span(callee), "Arrow functions are not constructors");
            }
            Expr::Function(f) if construct && (f.is_async || f.generator) => {
                self.warn(expr_span(callee), "Async and generator functions are not constructors");
            }
            Expr::Ident(id) => match self.lookup(&id.value) {
                Some(Binding {
                    kind: BindKind::Class,
                    ..
                }) if !construct => self.warn(
                    Some(id.span),
                    format!("Class constructor '{}' cannot be invoked without 'new'", id.value),
                ),
                _ => {}
            },
            _ => {}
        }
    }

    /// Queue an argument-count check for a call to a known function.
    fn queue_arity(&mut self, callee: &Expr, args: &[Expr]) {
        let Expr::Ident(id) = strip_parens(callee) else {
            return;
        };
        let Some(func) = self.lookup(&id.value).and_then(|b| b.func) else {
            return;
        };
        if !args.iter().any(|a| matches!(a, Expr::Spread(_))) {
            self.pending_calls.push((func, id.clone(), args.len()));
        }
    }

    fn check_arity(&mut self, arity: Arity, id: &Ident, n: usize) {
        let expected = match arity.max {
            Some(max) if max == arity.required => format!("{max}"),
            Some(max) if n > max => format!("at most {max}"),
            _ => format!("at least {}", arity.required),
        };
        if n < arity.required || arity.max.is_some_and(|max| n > max) {
            let plural = if expected.ends_with('1') { "" } else { "s" };
            self.warn(
                Some(id.span),
                format!("'{}' expects {expected} argument{plural}, but got {n}", id.value),
            );
        }
    }

    fn super_member(&mut self, span: TokenSpan) {
        if !matches!(
            self.this_kind(),
            FnKind::Method | FnKind::Constructor { .. } | FnKind::ClassField
        ) {
            self.error(Some(span), "'super' keyword unexpected here");
        }
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Literal(lit) => self.literal(lit),
            Expr::Ident(id) => self.reference(id),
            Expr::This(_) => {}
            Expr::Super(span) => self.error(Some(*span), "'super' keyword unexpected here"),
            Expr::Array(elems) => {
                for el in elems.iter().flatten() {
                    self.expr(el);
                }
            }
            Expr::Object(props) => {
                for prop in props {
                    match prop {
                        Property::KeyValue(key, value) => {
                            self.prop_key(key);
                            self.expr(value);
                        }
                        Property::Shorthand(id) => self.reference(id),
                        Property::Method(m) => self.method(m, FnKind::Method),
                        Property::Spread(value) => self.expr(value),
                    }
                }
            }
            Expr::Function(f) => {
                // A named function expression can refer to itself.
                self.push_scope(false);
                if let Some(name) = &f.name {
                    let func = self.func_id(&**f, &f.params);
                    self.declare(name, BindKind::Function, Some(func));
                }
                self.function(f, FnKind::Function);
                self.pop_scope();
            }
            Expr::Arrow(a) => self.arrow(a),
            Expr::Class(c) => self.class(c),
            Expr::Paren(inner) | Expr::Spread(inner) => self.expr(inner),
            Expr::Unary { op, value } => {
                if matches!(
                    **value,
                    Expr::Binary {
                        op: BinaryOp::Exp,
                        ..
                    }
                ) {
                    self.error(
                        expr_span(e),
                        "Unary operator used immediately before exponentiation expression; \
                         parentheses must be used to disambiguate operator precedence",
                    );
                }
                match op {
                    UnaryOp::Await if !self.ctx().is_async => self.error(
                        expr_span(value),
                        "await is only valid in async functions and the top level bodies of modules",
                    ),
                    UnaryOp::Delete if matches!(strip_parens(value), Expr::Ident(_)) => self.error(
                        expr_span(value),
                        "Delete of an unqualified identifier in strict mode",
                    ),
                    _ => {}
                }
                self.expr(value);
            }
            Expr::Update { prefix, target, .. } => {
                let message = if *prefix {
                    "Invalid left-hand side expression in prefix operation"
                } else {
                    "Invalid left-hand side expression in postfix operation"
                };
                self.simple_target(target, message);
            }
            Expr::Binary { left, op, right } => {
                let is_logical = |e: &Expr| {
                    matches!(
                        e,
                        Expr::Binary {
                            op: BinaryOp::And | BinaryOp::Or,
                            ..
                        }
                    )
                };
                let is_nullish = |e: &Expr| {
                    matches!(
                        e,
                        Expr::Binary {
                            op: BinaryOp::Nullish,
                            ..
                        }
                    )
                };
                let mixed = match op {
                    BinaryOp::Nullish => is_logical(left) || is_logical(right),
                    BinaryOp::And | BinaryOp::Or => is_nullish(left) || is_nullish(right),
                    _ => false,
                };
                if mixed {
                    self.error(
                        expr_span(e),
                        "Cannot mix '??' with '||' or '&&' without parentheses",
                    );
                }
                self.expr(left);
                self.expr(right);
            }
            Expr::Conditional {
                test,
                then,
                otherwise,
            } => {
                self.expr(test);
                self.expr(then);
                self.expr(otherwise);
            }
            Expr::Assign { target, op, value } => {
                match op {
                    AssignOp::Assign => self.assign_target(target),
                    _ => self.simple_target(target, "Invalid left-hand side in assignment"),
                }
                self.expr(value);
            }
            Expr::Sequence(es) => {
                for e in es {
                    self.expr(e);
                }
            }
            Expr::Member { object, .. } => match &**object {
                Expr::Super(span) => self.super_member(*span),
                _ => self.expr(object),
            },
            Expr::Index { object, index, .. } => {
                match &**object {
                    Expr::Super(span) => self.super_member(*span),
                    _ => self.expr(object),
                }
                self.expr(index);
            }
            Expr::Call { callee, args, .. } => {
                match &**callee {
                    Expr::Super(span) => {
                        if self.this_kind() != (FnKind::Constructor { derived: true }) {
                            self.error(
                                Some(*span),
                                "'super' call is only valid in constructors of derived classes",
                            );
                        }
                    }
                    _ => {
                        self.check_callee(callee, false);
                        self.queue_arity(callee, args);
                        self.expr(callee);
                    }
                }
                for a in args {
                    self.expr(a);
                }
            }
            Expr::New { callee, args } => {
                self.check_callee(callee, true);
                self.expr(callee);
                for a in args {
                    self.expr(a);
                }
            }
            Expr::Yield { span, value, .. } => {
                if !self.ctx().generator {
                    self.error(Some(*span), "yield is only valid in generator functions");
                }
                if let Some(v) = value {
                    self.expr(v);
                }
            }
        }
    }
}

/// `var` names declared anywhere in these statements, not crossing into
/// nested functions — they all hoist to the enclosing function scope.
fn collect_vars(s: &Statement, out: &mut Vec<Ident>) {
    let decl = |d: &VarDecl, out: &mut Vec<Ident>| {
        if matches!(d.kind, DeclKind::Var(_)) {
            out.extend(d.decls.iter().flat_map(|d| bound_names(&d.target)));
        }
    };
    match s {
        Statement::VarDecl(d) => decl(d, out),
        Statement::Block(body) => body.iter().for_each(|s| collect_vars(s, out)),
        Statement::If {
            then, otherwise, ..
        } => {
            collect_vars(then, out);
            if let Some(o) = otherwise {
                collect_vars(o, out);
            }
        }
        Statement::For { init, body, .. } => {
            if let Some(ForInit::VarDecl(d)) = init {
                decl(d, out);
            }
            collect_vars(body, out);
        }
        Statement::ForIn { left, body, .. } | Statement::ForOf { left, body, .. } => {
            if let ForLeft::Decl(DeclKind::Var(_), target) = left {
                out.extend(bound_names(target));
            }
            collect_vars(body, out);
        }
        Statement::While { body, .. }
        | Statement::DoWhile { body, .. }
        | Statement::Labeled { body, .. } => collect_vars(body, out),
        Statement::Try {
            block,
            handler,
            finalizer,
        } => {
            let handler = handler.iter().flat_map(|c| &c.body);
            for s in block.iter().chain(handler).chain(finalizer.iter().flatten()) {
                collect_vars(s, out);
            }
        }
        Statement::Switch { cases, .. } => {
            for s in cases.iter().flat_map(|c| &c.body) {
                collect_vars(s, out);
            }
        }
        Statement::Export {
            decl: ExportDecl::Decl(inner),
            ..
        } => collect_vars(inner, out),
        _ => {}
    }
}

/// Every identifier a pattern binds.
fn bound_names(p: &Pattern) -> Vec<Ident> {
    fn walk(p: &Pattern, out: &mut Vec<Ident>) {
        match p {
            Pattern::Ident(id) => out.push(id.clone()),
            Pattern::Array(elems) => elems.iter().flatten().for_each(|e| walk(e, out)),
            Pattern::Object(props) => {
                for prop in props {
                    match prop {
                        PatternProp::KeyValue(_, v) | PatternProp::Rest(v) => walk(v, out),
                        PatternProp::Shorthand(id, _) => out.push(id.clone()),
                    }
                }
            }
            Pattern::Default(target, _) | Pattern::Rest(target) => walk(target, out),
        }
    }
    let mut out = Vec::new();
    walk(p, &mut out);
    out
}

/// Whether a labeled statement (through nested labels) is a loop, and so a
/// valid `continue` target.
fn labels_loop(s: &Statement) -> bool {
    match s {
        Statement::For { .. }
        | Statement::ForIn { .. }
        | Statement::ForOf { .. }
        | Statement::While { .. }
        | Statement::DoWhile { .. } => true,
        Statement::Labeled { body, .. } => labels_loop(body),
        _ => false,
    }
}

fn strip_parens(e: &Expr) -> &Expr {
    match e {
        Expr::Paren(inner) => strip_parens(inner),
        _ => e,
    }
}

fn is_optional_chain(e: &Expr) -> bool {
    match e {
        Expr::Member {
            object, optional, ..
        }
        | Expr::Index {
            object, optional, ..
        } => *optional || is_optional_chain(object),
        Expr::Call {
            callee, optional, ..
        } => *optional || is_optional_chain(callee),
        _ => false,
    }
}

fn static_key_name(key: &PropKey) -> Option<&str> {
    match key {
        PropKey::Ident(id) => Some(&id.value),
        PropKey::Literal(Literal::String(s)) => Some(&s.value),
        _ => None,
    }
}

fn merge(acc: &mut Option<TokenSpan>, span: Option<TokenSpan>) {
    if let Some(s) = span {
        *acc = Some(match *acc {
            Some(a) => TokenSpan {
                start: a.start.min(s.start),
                end: a.end.max(s.end),
            },
            None => s,
        });
    }
}

/// The source range covered by an expression's tokens, as far as the AST
/// records them.
fn expr_span(e: &Expr) -> Option<TokenSpan> {
    let mut acc = None;
    match e {
        Expr::Literal(lit) => merge(&mut acc, Some(lit.span())),
        Expr::Ident(id) => merge(&mut acc, Some(id.span)),
        Expr::This(span) | Expr::Super(span) => merge(&mut acc, Some(*span)),
        Expr::Array(elems) => elems
            .iter()
            .flatten()
            .for_each(|el| merge(&mut acc, expr_span(el))),
        Expr::Object(props) => {
            for prop in props {
                let span = match prop {
                    Property::KeyValue(key, value) => {
                        merge(&mut acc, prop_key_span(key));
                        expr_span(value)
                    }
                    Property::Shorthand(id) => Some(id.span),
                    Property::Method(m) => prop_key_span(&m.key),
                    Property::Spread(value) => expr_span(value),
                };
                merge(&mut acc, span);
            }
        }
        Expr::Function(f) => {
            merge(&mut acc, f.name.as_ref().map(|n| n.span));
            f.params.iter().for_each(|p| merge(&mut acc, pattern_span(p)));
        }
        Expr::Arrow(a) => {
            a.params.iter().for_each(|p| merge(&mut acc, pattern_span(p)));
            if let ArrowBody::Expr(body) = &a.body {
                merge(&mut acc, expr_span(body));
            }
        }
        Expr::Class(c) => merge(&mut acc, c.name.as_ref().map(|n| n.span)),
        Expr::Paren(inner) | Expr::Spread(inner) => merge(&mut acc, expr_span(inner)),
        Expr::Unary { value, .. } => merge(&mut acc, expr_span(value)),
        Expr::Update { target, .. } => merge(&mut acc, expr_span(target)),
        Expr::Binary { left, right, .. } => {
            merge(&mut acc, expr_span(left));
            merge(&mut acc, expr_span(right));
        }
        Expr::Conditional {
            test, otherwise, ..
        } => {
            merge(&mut acc, expr_span(test));
            merge(&mut acc, expr_span(otherwise));
        }
        Expr::Assign { target, value, .. } => {
            merge(&mut acc, expr_span(target));
            merge(&mut acc, expr_span(value));
        }
        Expr::Sequence(es) => es.iter().for_each(|e| merge(&mut acc, expr_span(e))),
        Expr::Member {
            object, property, ..
        } => {
            merge(&mut acc, expr_span(object));
            merge(&mut acc, Some(property.span));
        }
        Expr::Index { object, index, .. } => {
            merge(&mut acc, expr_span(object));
            merge(&mut acc, expr_span(index));
        }
        Expr::Call { callee, args, .. } | Expr::New { callee, args } => {
            merge(&mut acc, expr_span(callee));
            args.iter().for_each(|a| merge(&mut acc, expr_span(a)));
        }
        Expr::Yield { span, value, .. } => {
            merge(&mut acc, Some(*span));
            merge(&mut acc, value.as_deref().and_then(expr_span));
        }
    }
    acc
}

fn prop_key_span(key: &PropKey) -> Option<TokenSpan> {
    match key {
        PropKey::Ident(id) => Some(id.span),
        PropKey::Literal(lit) => Some(lit.span()),
        PropKey::Computed(e) => expr_span(e),
    }
}

fn pattern_span(p: &Pattern) -> Option<TokenSpan> {
    let mut acc = None;
    match p {
        Pattern::Ident(id) => merge(&mut acc, Some(id.span)),
        Pattern::Array(elems) => elems
            .iter()
            .flatten()
            .for_each(|e| merge(&mut acc, pattern_span(e))),
        Pattern::Object(props) => {
            for prop in props {
                let span = match prop {
                    PatternProp::KeyValue(key, value) => {
                        merge(&mut acc, prop_key_span(key));
                        pattern_span(value)
                    }
                    PatternProp::Shorthand(id, _) => Some(id.span),
                    PatternProp::Rest(target) => pattern_span(target),
                };
                merge(&mut acc, span);
            }
        }
        Pattern::Default(target, default) => {
            merge(&mut acc, pattern_span(target));
            merge(&mut acc, expr_span(default));
        }
        Pattern::Rest(target) => merge(&mut acc, pattern_span(target)),
    }
    acc
}

fn left_span(left: &ForLeft) -> Option<TokenSpan> {
    match left {
        ForLeft::Decl(_, target) => pattern_span(target),
        ForLeft::Expr(e) => expr_span(e),
    }
}

#[cfg(test)]
mod tests {
    use chumsky::{IterParser, Parser, error::Rich, extra, prelude::end};

    use super::{Severity, validate};
    use crate::{lexer::tokenize, parser::JSCtx, token::TokenStruct};

    type TokInput<'src> = &'src [TokenStruct<'src>];
    type TokExtra<'src> = extra::Err<Rich<'src, TokenStruct<'src>>>;

    /// Each diagnostic as `E:` / `W:` plus the message and the source text it
    /// points at.
    fn check(src: &str) -> Vec<String> {
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("lex {src:?}: {e:?}"));
        let program = JSCtx::<TokInput, TokExtra>::build()
            .stmt
            .repeated()
            .collect::<Vec<_>>()
            .then_ignore(end())
            .parse(&tokens[..])
            .into_result()
            .unwrap_or_else(|e| panic!("parse {src:?}: {e:?}"));
        validate(&program)
            .into_iter()
            .map(|d| {
                let level = match d.severity {
                    Severity::Error => "E",
                    Severity::Warning => "W",
                };
                let at = d.span.map_or("?", |s| &src[s.start..s.end]);
                format!("{level}: {} @ {at}", d.message)
            })
            .collect()
    }

    fn clean(src: &str) {
        assert_eq!(check(src), Vec::<String>::new(), "{src}");
    }

    #[test]
    fn valid_programs_are_clean() {
        clean("const add = (a, b = 1) => a + b; add(1); add(1, 2)");
        clean("function f() { return g() } function g() { return f() }");
        clean("let x = 1; { let x = 2; x++ } x += 1");
        clean("var a = 1; var a = 2; function h(p) { var p }");
        clean("for (let i = 0; i < 3; i++) { if (i) continue; else break }");
        clean("outer: for (const x of [1]) { for (;;) { continue outer } }");
        clean("try { f() } catch ({ message }) { console.log(message) } function f() {}");
        clean("class A { constructor() { this.x = 1 } m() { return this.x } }");
        clean("class B extends Object { constructor() { super(); super.toString() } }");
        clean("async function f() { await 1; for await (const x of []) {} }");
        clean("function* g() { yield 1; yield* g() }");
        clean("const o = { get v() { return 1 }, set v(x) {}, m() { return super.m } }");
        clean("let a, b; [a, b] = [b, a]; ({ a, b: [b] } = {})");
        clean("switch (1) { case 1: let y = 1; break; default: }");
        clean("import { a as b } from 'm'; export { b as c }; export default b");
        clean("await Promise.resolve()");
        clean("(a => a)(1); new (class {})()");
        clean("function args() { return arguments.length } args(1, 2)");
    }

    #[test]
    fn invalid_assignment_targets() {
        assert_eq!(check("1 = 2"), ["E: Invalid left-hand side in assignment @ 1"]);
        assert_eq!(
            check("let a; a + 1 = 2"),
            ["E: Invalid left-hand side in assignment @ a + 1"]
        );
        assert_eq!(
            check("let a; a++ += 1"),
            ["E: Invalid left-hand side in assignment @ a"]
        );
        assert_eq!(
            check("let a; a?.b = 1"),
            ["E: Invalid left-hand side in assignment @ a?.b"]
        );
        assert_eq!(
            check("f()++; function f() {}"),
            ["E: Invalid left-hand side expression in postfix operation @ f"]
        );
        assert_eq!(
            check("let a; [...a, b] = []"),
            ["E: Rest element must be last element @ a", "W: 'b' is not defined @ b"]
        );
        assert_eq!(
            check("eval = 1"),
            ["E: Unexpected 'eval' as an assignment target in strict mode @ eval"]
        );
    }

    #[test]
    fn declarations() {
        assert_eq!(
            check("let a; let a"),
            ["E: Identifier 'a' has already been declared @ a"]
        );
        assert_eq!(
            check("let a; var a"),
            ["E: Identifier 'a' has already been declared @ a"]
        );
        assert_eq!(
            check("{ let b; { var b } }"),
            ["E: Identifier 'b' has already been declared @ b"]
        );
        assert_eq!(
            check("function f(p) { let p }"),
            ["E: Identifier 'p' has already been declared @ p"]
        );
        assert_eq!(
            check("const c"),
            ["E: Missing initializer in const declaration @ c"]
        );
        assert_eq!(
            check("let { d }"),
            ["E: Missing initializer in destructuring declaration @ d"]
        );
        assert_eq!(
            check("function f(a, a) {}"),
            ["E: Duplicate parameter name 'a' not allowed in this context @ a"]
        );
        assert_eq!(
            check("function f(...a, b) {}"),
            ["E: Rest parameter must be last formal parameter @ a"]
        );
        assert_eq!(
            check("let arguments = 1"),
            ["E: Unexpected 'arguments' as a binding name in strict mode @ arguments"]
        );
        assert_eq!(
            check("if (1) let x = 1"),
            ["E: Lexical declaration cannot appear in a single-statement context @ let"]
        );
        assert_eq!(
            check("while (0) function f() {}"),
            ["E: In strict mode code, functions can only be declared at top level or inside a block @ f"]
        );
        assert_eq!(check("x = 010"), [
            "W: 'x' is not defined @ x",
            "E: Octal literals and decimals with leading zeros are not allowed in strict mode @ 010",
        ]);
    }

    #[test]
    fn control_flow() {
        assert_eq!(check("return 1"), ["E: Illegal return statement @ return"]);
        assert_eq!(check("break"), ["E: Illegal break statement @ break"]);
        assert_eq!(
            check("switch (1) { case 1: continue }"),
            ["E: Illegal continue statement: no surrounding iteration statement @ continue"]
        );
        assert_eq!(
            check("for (;;) { break nowhere }"),
            ["E: Undefined label 'nowhere' @ nowhere"]
        );
        assert_eq!(
            check("blk: { for (;;) continue blk }"),
            ["E: Illegal continue statement: 'blk' does not denote an iteration statement @ blk"]
        );
        assert_eq!(
            check("a: a: ;"),
            ["E: Label 'a' has already been declared @ a"]
        );
        assert_eq!(
            check("for (;;) { (() => { break })() }"),
            ["E: Illegal break statement @ break"]
        );
        assert_eq!(
            check("switch (1) { default: default: }"),
            ["E: More than one default clause in switch statement @ default"]
        );
    }

    #[test]
    fn async_generators_super() {
        assert_eq!(
            check("function f() { await g() } async function g() {}"),
            ["E: await is only valid in async functions and the top level bodies of modules @ g"]
        );
        assert_eq!(
            check("async function f() { [1].map(x => await x) }"),
            ["E: await is only valid in async functions and the top level bodies of modules @ x"]
        );
        assert_eq!(
            check("function f() { yield 1 }"),
            ["E: yield is only valid in generator functions @ yield"]
        );
        assert_eq!(
            check("class A { constructor() { super() } }"),
            ["E: 'super' call is only valid in constructors of derived classes @ super"]
        );
        assert_eq!(
            check("function f() { return super.x }"),
            ["E: 'super' keyword unexpected here @ super"]
        );
    }

    #[test]
    fn classes_and_accessors() {
        assert_eq!(
            check("class A { constructor() {} constructor() {} }"),
            ["E: A class may only have one constructor @ constructor"]
        );
        assert_eq!(
            check("class A { get constructor() {} }"),
            ["E: Class constructor may not be an accessor, generator or async method @ constructor"]
        );
        assert_eq!(
            check("class A { static prototype() {} }"),
            ["E: Classes may not have a static property named 'prototype' @ prototype"]
        );
        assert_eq!(
            check("({ get v(x) {} })"),
            ["E: Getter must not have any formal parameters @ v"]
        );
        assert_eq!(
            check("({ set v() {} })"),
            ["E: Setter must have exactly one formal parameter @ v"]
        );
    }

    #[test]
    fn operators() {
        assert_eq!(
            check("-2 ** 2"),
            ["E: Unary operator used immediately before exponentiation expression; \
              parentheses must be used to disambiguate operator precedence @ 2 ** 2"]
        );
        clean("(-2) ** 2");
        assert_eq!(
            check("let a, b, c; a ?? b || c"),
            ["E: Cannot mix '??' with '||' or '&&' without parentheses @ a ?? b || c"]
        );
        clean("let a, b, c; (a ?? b) || c");
        assert_eq!(
            check("let x; delete x"),
            ["E: Delete of an unqualified identifier in strict mode @ x"]
        );
    }

    #[test]
    fn function_calls() {
        assert_eq!(
            check("function f(a, b) {} f(1)"),
            ["W: 'f' expects 2 arguments, but got 1 @ f"]
        );
        assert_eq!(
            check("const g = (a, b = 2) => a; g(); g(1, 2, 3)"),
            [
                "W: 'g' expects at least 1 argument, but got 0 @ g",
                "W: 'g' expects at most 2 arguments, but got 3 @ g",
            ]
        );
        clean("function v(...xs) {} v(); v(1, 2, 3)");
        clean("function s(a, b) {} s(...[1, 2])");
        assert_eq!(
            check("1()"),
            ["W: Cannot call this expression: '1' is not a function @ 1"]
        );
        assert_eq!(
            check("'str'.length()"),
            Vec::<String>::new(),
            "member calls are not checked"
        );
        assert_eq!(
            check("class K {} K()"),
            ["W: Class constructor 'K' cannot be invoked without 'new' @ K"]
        );
        assert_eq!(
            check("new (x => 1)()"),
            ["W: Arrow functions are not constructors @ x => 1"]
        );
    }

    #[test]
    fn variable_names() {
        assert_eq!(check("y + 1"), ["W: 'y' is not defined @ y"]);
        assert_eq!(
            check("const k = 1; k = 2"),
            ["W: Assignment to constant variable 'k' @ k"]
        );
        assert_eq!(
            check("import x from 'm'; x = 1"),
            ["W: Assignment to imported binding 'x' @ x"]
        );
        assert_eq!(
            check("function f() { return inner } { let inner }"),
            ["W: 'inner' is not defined @ inner"]
        );
        assert_eq!(check("console.log(undefined, NaN)"), Vec::<String>::new());
    }

    #[test]
    fn modules() {
        assert_eq!(
            check("export { missing }"),
            ["E: Export 'missing' is not defined @ missing"]
        );
        assert_eq!(
            check("export const a = 1; export { a }"),
            ["E: Duplicate export of 'a' @ a"]
        );
        assert_eq!(
            check("{ import x from 'm' }"),
            ["E: import declarations may only appear at top level of a module @ import"]
        );
    }
}
