use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use rjs_core::{
    expr::{AssignOp, BinaryOp, Expr, UnaryOp, UpdateOp},
    function::Function,
    ident::Ident,
    literal::Literal,
    pattern::{Pattern, PatternProp},
    run_parser,
    statement::{DeclKind, ForInit, ForLeft, Statement, SwitchCase, VarDecl},
};

use crate::{
    error::JSError,
    executor::{
        instr::{
            Binary, Call, Declare, GetAttr, GetIndex, Hoist, INITIAL_STATS, INSTRUCTION_KINDS,
            Init, Instruction, Iterable, Jump, JumpIf, JumpPos, MakeFunction, Operand, Push,
            Return, Set, SetIndex, Statistics, Unary, UnaryOperator,
        },
        scope::{Scope, VariableType},
    },
    primitive::{JsValue, func::Func, jstack::JSStack},
};

#[derive(Debug, Clone)]
pub enum InstructionType {
    Instruction(Vec<LabelledInstruction>),
    Value(Operand),
}

#[derive(Debug, Clone)]
pub struct LabelledInstruction {
    label: Option<String>,
    instruction: Instruction,
}

#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
pub enum Register {
    AX,
    BX,
    CX,
    DX,
}

const REGISTER_COUNT: usize = 4;

/// How deeply JS functions may call each other before a call throws a
/// RangeError, like Node's limit (about 10,000). Calls are frames on the
/// heap, not Rust recursion, so this only bounds runaway recursion.
pub const MAX_CALL_DEPTH: usize = 10_000;

/// A statement `break` or `continue` can leave, while compiling its body.
#[derive(Clone)]
struct JumpTarget {
    /// The labels naming it (`outer: for ...`).
    labels: Vec<String>,
    /// Whether a `break` without a label can leave it: loops and `switch`,
    /// but not other labeled statements.
    bare_break: bool,
    /// Where `break` goes: just after the statement.
    break_to: String,
    /// Where `continue` goes, for a loop: the next iteration.
    continue_to: Option<String>,
    /// Block scopes open at `break_to` and `continue_to`.
    scope_depth: usize,
}

/// A call in progress, saved while the function it called runs.
struct Frame {
    /// The caller's instructions; `None` for those passed to `run_frame`.
    code: Option<Rc<Vec<Instruction>>>,
    /// Where the caller resumes: the instruction after its `Call`.
    pc: usize,
    /// The caller's innermost scope.
    scope: Rc<Scope>,
    /// The caller's enclosing block scopes, restored by `EndScope`.
    outer: Vec<Rc<Scope>>,
}

#[derive(Clone)]
pub struct JSEngine {
    pub scope: Rc<Scope>,
    /// Indexed by `Register as usize`.
    pub registers: [JsValue; REGISTER_COUNT],
    pub stack: JSStack,
    /// Counter for jump labels during compilation (`__L0`, `__L1`, ...).
    labels: usize,
    /// Counter for compiler temporaries (`__t0`, `__t1`, ...). Restarts at
    /// each statement, since a temporary never outlives its statement.
    temps: usize,
    /// Print the parsed program, diagnostics and emitter tracing. Off for
    /// runs that should only show what the program itself logs.
    pub verbose: bool,
    /// Per-instruction counts and times for the last `run`, gathered in
    /// verbose mode.
    pub stats: [Statistics; INSTRUCTION_KINDS],
    /// The statements `break` and `continue` can reach from the code being
    /// compiled, innermost last.
    jump_targets: Vec<JumpTarget>,
    /// Block scopes open at this point of the code being compiled. A jump
    /// out of blocks ends each scope it leaves.
    scope_depth: usize,
    /// The labels of the loop or `switch` about to be compiled.
    pending_labels: Vec<String>,
    /// JS function calls in progress; see `MAX_CALL_DEPTH`.
    call_depth: usize,
    /// A JS function the last `Call` instruction asked to run: its code and
    /// the scope to run it in. `run_frame` starts it before the next
    /// instruction.
    pub(crate) pending_call: Option<(Rc<Vec<Instruction>>, Rc<Scope>)>,
}

#[allow(clippy::new_without_default)]
impl JSEngine {
    pub fn new() -> Self {
        let registers = std::array::from_fn(|_| JsValue::Undefined);
        Self {
            scope: Scope::create_default(),
            registers,
            stack: JSStack::default(),
            labels: 0,
            temps: 0,
            verbose: true,
            stats: INITIAL_STATS,
            jump_targets: vec![],
            scope_depth: 0,
            pending_labels: vec![],
            call_depth: 0,
            pending_call: None,
        }
    }

    /// Parse, compile and run `src`.
    pub fn exec(&mut self, src: &str) -> Result<(), JSError> {
        let program = self.parse(src)?;
        let instrs = self.compile(&program)?;
        self.run(&instrs)
    }

    /// Parse and validate `src`.
    pub fn parse(&self, src: &str) -> Result<Vec<Statement>, JSError> {
        let res = run_parser(src);
        if !res.errors.is_empty() {
            return Err(JSError::BadJavascript(res.errors));
        }
        if self.verbose {
            for s in &res.program {
                println!("{}", s);
            }
            for s in res.diagnostics {
                println!("Warning: {}", s);
            }
        }
        Ok(res.program)
    }

    /// Compile a parsed program to instructions. Jumps are resolved to
    /// instruction positions, so the result doesn't depend on this engine.
    pub fn compile(&mut self, program: &[Statement]) -> Result<Vec<Instruction>, JSError> {
        link(self.emit_code(program)?)
    }

    /// Run compiled instructions in the engine's global scope.
    pub fn run(&mut self, instrs: &[Instruction]) -> Result<(), JSError> {
        self.stats = INITIAL_STATS;
        // An error ends the run wherever it happens, so start counting afresh.
        self.call_depth = 0;
        let scope = self.scope.clone();
        self.run_frame(scope, instrs)?;
        if self.verbose {
            println!("{:?}", self.stats);
        }
        Ok(())
    }

    /// Runs `instrs` starting in `scope`, until they end or execute a
    /// `return`. Returns whether a `return` ended them.
    ///
    /// Calls to JS functions made along the way run in this same loop: the
    /// caller's position is saved as a `Frame` and the callee's code takes
    /// over until it returns, so JS recursion doesn't recurse in Rust.
    pub(crate) fn run_frame(
        &mut self,
        scope: Rc<Scope>,
        instrs: &[Instruction],
    ) -> Result<bool, JSError> {
        let mut frames: Vec<Frame> = vec![];
        // The frame being run. `code` is `None` while running `instrs`.
        let mut code: Option<Rc<Vec<Instruction>>> = None;
        let mut scope = scope;
        let mut outer: Vec<Rc<Scope>> = vec![];
        let mut pc = 0;
        loop {
            let current = code.as_deref().map_or(instrs, Vec::as_slice);
            let returned = match current.get(pc) {
                // Falling off the end: a function gives `undefined`.
                None => false,
                Some(Instruction::BeginScope) => {
                    let block = Scope::create(scope.clone());
                    outer.push(std::mem::replace(&mut scope, block));
                    pc += 1;
                    continue;
                }
                Some(Instruction::EndScope) => {
                    scope = outer
                        .pop()
                        .ok_or(JSError::UnknownError("Could not find scope to pop.".into()))?;
                    pc += 1;
                    continue;
                }
                Some(instr) => {
                    let jump = instr.exec(self, &scope)?;
                    match Self::next_pc(pc, jump)? {
                        // `return`
                        None => true,
                        Some(next) => {
                            pc = next;
                            if let Some((callee, callee_scope)) = self.pending_call.take() {
                                self.enter_call()?;
                                frames.push(Frame {
                                    code: code.replace(callee),
                                    pc,
                                    scope: std::mem::replace(&mut scope, callee_scope),
                                    outer: std::mem::take(&mut outer),
                                });
                                pc = 0;
                            }
                            continue;
                        }
                    }
                }
            };
            // The frame has ended; its result (if any) is in AX.
            let Some(caller) = frames.pop() else {
                return Ok(returned);
            };
            if !returned {
                self.set_register(Register::AX, JsValue::Undefined);
            }
            self.call_depth -= 1;
            (code, pc, scope, outer) = (caller.code, caller.pc, caller.scope, caller.outer);
        }
    }

    /// Where to go after the instruction at `pc` asked for `jump`; `None`
    /// for a `return`.
    fn next_pc(pc: usize, jump: Option<JumpPos>) -> Result<Option<usize>, JSError> {
        Ok(match jump {
            None => Some(pc + 1),
            Some(JumpPos::Global(g)) => Some(g),
            Some(JumpPos::Relative(r)) => Some((pc as isize + r) as usize),
            Some(JumpPos::Exit) => None,
            Some(JumpPos::Label(l)) => {
                return Err(JSError::UnknownError(format!("unlinked label {l}")));
            }
        })
    }

    /// Counts a JS call starting, failing once there are too many.
    fn enter_call(&mut self) -> Result<(), JSError> {
        if self.call_depth >= MAX_CALL_DEPTH {
            return Err(JSError::RangeError(
                "Maximum call stack size exceeded".to_string(),
            ));
        }
        self.call_depth += 1;
        Ok(())
    }

    /// Calls a JS function from Rust (a built-in calling a function it was
    /// given) and returns its result. Runs a nested `run_frame`, so unlike
    /// calls from JS this does recurse in Rust.
    pub fn call_function(&mut self, func: &Func, args: Vec<JsValue>) -> Result<JsValue, JSError> {
        let (code, scope) = func.enter(JsValue::Undefined, args)?;
        self.enter_call()?;
        let returned = self.run_frame(scope, &code)?;
        self.call_depth -= 1;
        Ok(if returned {
            self.get_register(Register::AX)
        } else {
            JsValue::Undefined
        })
    }

    fn emit_code(&mut self, stmts: &[Statement]) -> Result<Vec<LabelledInstruction>, JSError> {
        self.labels = 0;
        let mut instrs = hoist_declarations(stmts, true);
        instrs.extend(self.emit_function_declarations(stmts)?);
        for st in stmts {
            instrs.extend(self.emit_statement(st)?);
        }
        Ok(instrs)
    }

    fn emit_statement(&mut self, st: &Statement) -> Result<Vec<LabelledInstruction>, JSError> {
        // Temporaries hold values only within one statement, so each
        // statement can reuse the same few names. Nested statements (an if's
        // branches) start after the enclosing statement's temporaries are
        // no longer needed.
        self.temps = 0;
        match st {
            Statement::VarDecl(v) => Ok(self.emit_vardecl(v)?),
            Statement::Expr(ex) => Ok(match self.emit_exprs(ex, None)? {
                InstructionType::Instruction(v) => v,
                // Reading a variable still fails if it isn't declared.
                InstructionType::Value(var @ Operand::Var(_)) => {
                    vec![self.set_instr(Operand::Register(Register::AX), var).into()]
                }
                InstructionType::Value(_) => vec![],
            }),
            Statement::Empty | Statement::Debugger => Ok(vec![]),
            Statement::Block(b) => Ok(self.emit_block(b)?),
            Statement::If {
                test,
                then,
                otherwise,
            } => Ok(self.emit_if(test, then, otherwise)?),
            Statement::For {
                init,
                test,
                update,
                body,
            } => Ok(self.emit_for(init, test, update, body)?),
            Statement::While { test, body } => Ok(self.emit_while(test, body)?),
            Statement::DoWhile { body, test } => Ok(self.emit_do_while(body, test)?),
            Statement::ForOf {
                is_await,
                left,
                right,
                body,
            } => Ok(self.emit_for_of(*is_await, left, right, body)?),
            Statement::Switch {
                discriminant,
                cases,
            } => Ok(self.emit_switch(discriminant, cases)?),
            Statement::Break { span: _, label } => Ok(self.emit_jump_out(label, false)?),
            Statement::Continue { span: _, label } => Ok(self.emit_jump_out(label, true)?),
            Statement::Labeled { label, body } => Ok(self.emit_labeled(label, body)?),
            Statement::Function(_f) => Ok(vec![]),
            Statement::Return { span: _, value } => Ok(self.emit_return(value)?),
            Statement::ForIn { .. } => Err(JSError::NotSupported("for-in".into())),
            Statement::Throw(_) | Statement::Try { .. } => {
                Err(JSError::NotSupported("exceptions (throw, try)".into()))
            }
            Statement::Class(_) => Err(JSError::NotSupported("classes".into())),
            Statement::Import(_) | Statement::Export { .. } => {
                Err(JSError::NotSupported("modules (import, export)".into()))
            }
        }
    }

    fn emit_return(&mut self, value: &Option<Expr>) -> Result<Vec<LabelledInstruction>, JSError> {
        let mut ins = vec![];
        let value = match value {
            Some(v) => self.emit_operand(v, &mut ins)?,
            None => Operand::Value(JsValue::Undefined),
        };
        ins.push(Instruction::Return(Return { value: Some(value) }).into());
        Ok(ins)
    }

    /// Creates this scope's function declarations. Runs after every name in
    /// the scope is hoisted, so functions can call each other.
    fn emit_function_declarations(
        &mut self,
        stmts: &[Statement],
    ) -> Result<Vec<LabelledInstruction>, JSError> {
        let mut ins = vec![];
        for st in stmts {
            if let Statement::Function(f) = st
                && let Some(name) = &f.name
            {
                let function = self.emit_function(f, name.value.clone())?;
                ins.push(
                    Instruction::MakeFunction(MakeFunction {
                        function,
                        output: Operand::Register(Register::AX),
                    })
                    .into(),
                );
                ins.push(
                    Instruction::Init(Init {
                        name: name.value.clone(),
                        value: Operand::Register(Register::AX),
                    })
                    .into(),
                );
            }
        }
        Ok(ins)
    }

    /// Compiles a function's body into its own linked instructions. The body
    /// starts by binding the parameters the way
    /// `let [a, b = 2, ...rest] = arguments` would, so defaults, rest
    /// parameters and destructuring all work; then it hoists its own
    /// declarations like a program does.
    fn emit_function(&mut self, func: &Function, name: String) -> Result<Func, JSError> {
        if func.is_async {
            return Err(JSError::NotSupported("async  functions".into()));
        }
        // The body is compiled in the middle of the enclosing code; keep its
        // temporaries' numbering apart.
        let saved_temps = self.temps;
        self.temps = 0;
        // `break` and `continue` can't leave a function, and its code runs in
        // a frame with no block scopes open yet.
        let saved_targets = std::mem::take(&mut self.jump_targets);
        let saved_depth = std::mem::replace(&mut self.scope_depth, 0);
        let saved_labels = std::mem::take(&mut self.pending_labels);
        let params = Pattern::Array(func.params.iter().cloned().map(Some).collect());
        let mut ins: Vec<LabelledInstruction> = binding_names(&params)
            .into_iter()
            .map(|name| {
                Instruction::Hoist(Hoist {
                    name,
                    kind: VariableType::Any,
                })
                .into()
            })
            .collect();
        ins.extend(self.bind_pattern(&params, Operand::Var("arguments".into()))?);
        ins.extend(hoist_declarations(&func.body, true));
        ins.extend(self.emit_function_declarations(&func.body)?);
        for s in &func.body {
            ins.extend(self.emit_statement(s)?);
        }
        self.temps = saved_temps;
        self.jump_targets = saved_targets;
        self.scope_depth = saved_depth;
        self.pending_labels = saved_labels;
        // `length` counts the parameters before the first default or rest.
        let arg_count = func
            .params
            .iter()
            .take_while(|p| !matches!(p, Pattern::Default(..) | Pattern::Rest(_)))
            .count();
        Ok(Func::compiled(
            &name,
            link(ins)?,
            func.generator,
            func.is_async,
            arg_count,
        ))
    }

    fn emit_for(
        &mut self,
        init: &Option<ForInit>,
        test: &Option<Expr>,
        update: &Option<Expr>,
        body: &Statement,
    ) -> Result<Vec<LabelledInstruction>, JSError> {
        let mut ins = vec![Instruction::BeginScope.into()];
        self.scope_depth += 1;
        if let Some(ForInit::VarDecl(v)) = init {
            ins.extend(hoist_declarations(&[Statement::VarDecl(v.clone())], false));
        }
        if let Some(init) = init {
            let init = match init {
                ForInit::Expr(e) => self.emit_exprs(e, None)?,
                ForInit::VarDecl(v) => self.emit_vardecl_expr(v)?,
            };
            ins.extend(self.extract_instructions(init, None));
        }
        let (start, next, end) = (self.new_label(), self.new_label(), self.new_label());
        ins.push(Self::label(start.clone()));
        if let Some(test) = test {
            ins.extend(self.emit_loop_exit(test, &end)?);
        }
        ins.extend(self.emit_loop_body(body, &end, &next)?);
        ins.push(Self::label(next));
        if let Some(update) = update {
            let update = self.emit_exprs(update, None)?;
            ins.extend(self.extract_instructions(update, None));
        }
        ins.push(Self::jump_to(start));
        ins.push(Self::label(end));
        ins.push(Instruction::EndScope.into());
        self.scope_depth -= 1;
        Ok(ins)
    }

    fn emit_while(
        &mut self,
        test: &Expr,
        body: &Statement,
    ) -> Result<Vec<LabelledInstruction>, JSError> {
        let (start, end) = (self.new_label(), self.new_label());
        let mut ins = vec![Self::label(start.clone())];
        ins.extend(self.emit_loop_exit(test, &end)?);
        ins.extend(self.emit_loop_body(body, &end, &start)?);
        ins.push(Self::jump_to(start));
        ins.push(Self::label(end));
        Ok(ins)
    }

    fn emit_do_while(
        &mut self,
        body: &Statement,
        test: &Expr,
    ) -> Result<Vec<LabelledInstruction>, JSError> {
        let (start, next, end) = (self.new_label(), self.new_label(), self.new_label());
        let mut ins = vec![Self::label(start.clone())];
        ins.extend(self.emit_loop_body(body, &end, &next)?);
        ins.push(Self::label(next));
        let value = self.emit_operand(test, &mut ins)?;
        ins.push(
            Instruction::JumpIf(JumpIf {
                if_true: true,
                value,
                pos: JumpPos::Label(start),
            })
            .into(),
        );
        ins.push(Self::label(end));
        Ok(ins)
    }

    /// `for (left of right) body`, over the elements of an array or the
    /// characters of a string. The length is read every iteration, so
    /// elements pushed by the body are visited too, as in JS.
    fn emit_for_of(
        &mut self,
        is_await: bool,
        left: &ForLeft,
        right: &Expr,
        body: &Statement,
    ) -> Result<Vec<LabelledInstruction>, JSError> {
        if is_await {
            return Err(JSError::NotSupported("for await".into()));
        }
        let ax = Operand::Register(Register::AX);
        let mut ins = vec![Instruction::BeginScope.into()];
        self.scope_depth += 1;
        if let ForLeft::Decl(kind, target) = left
            && !matches!(kind, DeclKind::Var(_))
        {
            let kind = VariableType::from(kind.clone());
            ins.extend(binding_names(target).into_iter().map(|name| {
                LabelledInstruction::from(Instruction::Hoist(Hoist {
                    name,
                    kind: kind.clone(),
                }))
            }));
        }
        // The array and position live for the whole loop, so they can't be
        // statement temporaries, which the body's statements reuse.
        let (start, next, end) = (self.new_label(), self.new_label(), self.new_label());
        let items = format!("{start}_items");
        let index = format!("{start}_index");
        let value = self.emit_operand(right, &mut ins)?;
        ins.push(
            Instruction::Iterable(Iterable {
                value,
                output: ax.clone(),
                pc: None,
            })
            .into(),
        );
        ins.push(Self::declare_instr(
            items.clone(),
            VariableType::Any,
            ax.clone(),
        ));
        ins.push(Self::declare_instr(
            index.clone(),
            VariableType::Any,
            Operand::Value(0.0.into()),
        ));
        ins.push(Self::label(start.clone()));
        ins.push(
            Instruction::GetAttr(GetAttr {
                object: Operand::Var(items.clone()),
                attrib: "length".into(),
                output: ax.clone(),
                obj_output: Operand::Register(Register::BX),
            })
            .into(),
        );
        ins.push(
            Instruction::Binary(Binary {
                left: Operand::Var(index.clone()),
                op: BinaryOp::Lt,
                right: ax.clone(),
                output: Some(Register::AX),
            })
            .into(),
        );
        ins.push(
            Instruction::JumpIf(JumpIf {
                if_true: false,
                value: ax.clone(),
                pos: JumpPos::Label(end.clone()),
            })
            .into(),
        );
        ins.push(
            Instruction::GetIndex(GetIndex {
                object: Operand::Var(items),
                index: Operand::Var(index.clone()),
                output: ax.clone(),
            })
            .into(),
        );
        match left {
            ForLeft::Decl(_, target) => ins.extend(self.bind_pattern(target, ax)?),
            ForLeft::Expr(Expr::Ident(ident)) => ins.push(
                Instruction::Set(Set {
                    l_value: Operand::Var(ident.value.clone()),
                    r_value: ax,
                })
                .into(),
            ),
            ForLeft::Expr(_) => {
                return Err(JSError::NotSupported(
                    "for-of assigning to a property or pattern".into(),
                ));
            }
        }
        ins.extend(self.emit_loop_body(body, &end, &next)?);
        ins.push(Self::label(next));
        ins.push(Self::unary_instr(
            UnaryOperator::Increment,
            Operand::Var(index.clone()),
            Operand::Var(index),
        ));
        ins.push(Self::jump_to(start));
        ins.push(Self::label(end));
        ins.push(Instruction::EndScope.into());
        self.scope_depth -= 1;
        Ok(ins)
    }

    /// `switch`: the cases' tests, in order, each jumping to its case's body
    /// on a match (`===`), then the bodies one after another so that a case
    /// without `break` falls through. The bodies share one block scope.
    fn emit_switch(
        &mut self,
        discriminant: &Expr,
        cases: &[SwitchCase],
    ) -> Result<Vec<LabelledInstruction>, JSError> {
        let mut ins = vec![];
        let value = self.emit_operand(discriminant, &mut ins)?;
        let end = self.new_label();
        // Kept for every test, so not a temporary.
        let discriminant = format!("{end}_switch");
        ins.push(Self::declare_instr(
            discriminant.clone(),
            VariableType::Any,
            value,
        ));

        let body: Vec<Statement> = cases.iter().flat_map(|c| c.body.iter().cloned()).collect();
        let mut declarations = hoist_declarations(&body, false);
        declarations.extend(self.emit_function_declarations(&body)?);
        let own_scope = !declarations.is_empty();
        if own_scope {
            ins.push(Instruction::BeginScope.into());
            ins.extend(declarations);
            self.scope_depth += 1;
        }

        let case_labels: Vec<String> = cases.iter().map(|_| self.new_label()).collect();
        for (case, label) in cases.iter().zip(&case_labels) {
            let Some(test) = &case.test else { continue };
            let test = self.emit_operand(test, &mut ins)?;
            ins.push(
                Instruction::Binary(Binary {
                    left: Operand::Var(discriminant.clone()),
                    op: BinaryOp::StrictEq,
                    right: test,
                    output: Some(Register::AX),
                })
                .into(),
            );
            ins.push(
                Instruction::JumpIf(JumpIf {
                    if_true: true,
                    value: Operand::Register(Register::AX),
                    pos: JumpPos::Label(label.clone()),
                })
                .into(),
            );
        }
        let default = cases.iter().position(|c| c.test.is_none());
        ins.push(Self::jump_to(
            default.map_or_else(|| end.clone(), |i| case_labels[i].clone()),
        ));

        let target = JumpTarget {
            labels: std::mem::take(&mut self.pending_labels),
            bare_break: true,
            break_to: end.clone(),
            continue_to: None,
            scope_depth: self.scope_depth,
        };
        self.jump_targets.push(target);
        let bodies = (|| {
            let mut bodies = vec![];
            for (case, label) in cases.iter().zip(case_labels) {
                bodies.push(Self::label(label));
                for st in &case.body {
                    bodies.extend(self.emit_statement(st)?);
                }
            }
            Ok::<_, JSError>(bodies)
        })();
        self.jump_targets.pop();
        ins.extend(bodies?);
        ins.push(Self::label(end));
        if own_scope {
            ins.push(Instruction::EndScope.into());
            self.scope_depth -= 1;
        }
        Ok(ins)
    }

    /// `label: body`. A loop or `switch` takes the label as its own, so
    /// `continue label` reaches its next iteration; any other statement can
    /// only be left with `break label`.
    fn emit_labeled(
        &mut self,
        label: &Ident,
        body: &Statement,
    ) -> Result<Vec<LabelledInstruction>, JSError> {
        self.pending_labels.push(label.value.clone());
        if matches!(
            body,
            Statement::For { .. }
                | Statement::ForOf { .. }
                | Statement::ForIn { .. }
                | Statement::While { .. }
                | Statement::DoWhile { .. }
                | Statement::Switch { .. }
                | Statement::Labeled { .. }
        ) {
            return self.emit_statement(body);
        }
        let end = self.new_label();
        let target = JumpTarget {
            labels: std::mem::take(&mut self.pending_labels),
            bare_break: false,
            break_to: end.clone(),
            continue_to: None,
            scope_depth: self.scope_depth,
        };
        self.jump_targets.push(target);
        let ins = self.emit_statement(body);
        self.jump_targets.pop();
        let mut ins = ins?;
        ins.push(Self::label(end));
        Ok(ins)
    }

    /// `break` or `continue`, optionally to a label: end each block scope
    /// being left, then jump.
    fn emit_jump_out(
        &mut self,
        label: &Option<Ident>,
        is_continue: bool,
    ) -> Result<Vec<LabelledInstruction>, JSError> {
        let bad = |message: String| JSError::BadJavascript(vec![message]);
        let keyword = if is_continue { "continue" } else { "break" };
        let target = match label {
            Some(label) => self
                .jump_targets
                .iter()
                .rev()
                .find(|t| t.labels.contains(&label.value))
                .ok_or_else(|| bad(format!("Undefined label '{}'", label.value)))?,
            None => self
                .jump_targets
                .iter()
                .rev()
                .find(|t| {
                    if is_continue {
                        t.continue_to.is_some()
                    } else {
                        t.bare_break
                    }
                })
                .ok_or_else(|| bad(format!("Illegal {keyword} statement")))?,
        };
        let to = if is_continue {
            target.continue_to.clone().ok_or_else(|| {
                bad(format!(
                    "Illegal continue statement: '{}' does not denote an iteration statement",
                    label.as_ref().map_or("", |l| l.value.as_str())
                ))
            })?
        } else {
            target.break_to.clone()
        };
        let mut ins: Vec<LabelledInstruction> = (target.scope_depth..self.scope_depth)
            .map(|_| Instruction::EndScope.into())
            .collect();
        ins.push(Self::jump_to(to));
        Ok(ins)
    }

    /// Jumps to `end` when `test` is falsy.
    fn emit_loop_exit(
        &mut self,
        test: &Expr,
        end: &str,
    ) -> Result<Vec<LabelledInstruction>, JSError> {
        let mut ins = vec![];
        let value = self.emit_operand(test, &mut ins)?;
        ins.push(
            Instruction::JumpIf(JumpIf {
                if_true: false,
                value,
                pos: JumpPos::Label(end.to_string()),
            })
            .into(),
        );
        Ok(ins)
    }

    /// A loop's body, where `break` goes to `end` and `continue` to `next`.
    fn emit_loop_body(
        &mut self,
        body: &Statement,
        end: &str,
        next: &str,
    ) -> Result<Vec<LabelledInstruction>, JSError> {
        let target = JumpTarget {
            labels: std::mem::take(&mut self.pending_labels),
            bare_break: true,
            break_to: end.to_string(),
            continue_to: Some(next.to_string()),
            scope_depth: self.scope_depth,
        };
        self.jump_targets.push(target);
        let body = self.emit_statement(body);
        self.jump_targets.pop();
        body
    }

    /// A jump target: a `Noop` carrying `label`.
    fn label(label: String) -> LabelledInstruction {
        LabelledInstruction {
            label: Some(label),
            instruction: Instruction::Noop,
        }
    }

    fn jump_to(label: String) -> LabelledInstruction {
        Instruction::Jump(Jump {
            pos: JumpPos::Label(label),
        })
        .into()
    }

    fn extract_instructions(
        &self,
        ins: InstructionType,
        output: Option<Operand>,
    ) -> Vec<LabelledInstruction> {
        let mut res = vec![];
        match ins {
            InstructionType::Instruction(ins) => {
                res.extend(ins);
            }
            InstructionType::Value(v) => {
                if let Some(output) = output {
                    res.push(self.set_instr(output, v).into())
                } else {
                    res.push(self.set_instr(Operand::Register(Register::AX), v).into())
                }
            }
        }
        res
    }

    /// The name is already hoisted to the start of its scope, so a declaration
    /// only runs its initializer, here at its own line.
    fn emit_vardecl(&mut self, decl: &VarDecl) -> Result<Vec<LabelledInstruction>, JSError> {
        let mut instrs = vec![];
        let is_var = matches!(decl.kind, DeclKind::Var(_));
        for d in &decl.decls {
            let value = match &d.init {
                // `var x;` leaves an existing value alone.
                None if is_var => continue,
                None => Operand::Value(JsValue::Undefined),
                Some(init) => self.emit_operand(init, &mut instrs)?,
            };
            instrs.extend(self.bind_pattern(&d.target, value)?);
        }
        Ok(instrs)
    }

    /// Emits `ex`, appending any instructions it needs to `instrs`, and
    /// returns where its value ends up.
    fn emit_operand(
        &mut self,
        ex: &Expr,
        instrs: &mut Vec<LabelledInstruction>,
    ) -> Result<Operand, JSError> {
        Ok(match self.emit_exprs(ex, None)? {
            InstructionType::Value(v) => v,
            InstructionType::Instruction(ins) => {
                instrs.extend(ins);
                Operand::Register(Register::AX)
            }
        })
    }

    /// Binds the names in `pattern` to `value`. The names are already
    /// hoisted, so binding means initializing them. Every case reads `value`
    /// in its first instruction, so it may be a register.
    ///
    /// - `x`: initialize `x` to the value.
    /// - `target = default`: use `default` instead when the value is
    ///   `undefined`, then bind `target`.
    /// - `[a, , b, ...rest]`: element 0 is bound to `a`, element 1 is
    ///   skipped, element 2 is bound to `b`, and the elements from 3 on are
    ///   collected into a new array bound to `rest`. Each element can itself
    ///   be any pattern.
    fn bind_pattern(
        &mut self,
        pattern: &Pattern,
        value: Operand,
    ) -> Result<Vec<LabelledInstruction>, JSError> {
        let mut instrs = vec![];
        match pattern {
            Pattern::Ident(id) => instrs.push(
                Instruction::Init(Init {
                    name: id.value.clone(),
                    value,
                })
                .into(),
            ),
            Pattern::Default(target, default) => {
                // tmp = value; if (tmp === undefined) tmp = default;
                let tmp = self.new_temp();
                instrs.push(Self::declare_instr(tmp.clone(), VariableType::Any, value));
                let done = self.new_label();
                instrs.push(
                    Instruction::Binary(Binary {
                        left: Operand::Var(tmp.clone()),
                        op: BinaryOp::StrictEq,
                        right: Operand::Value(JsValue::Undefined),
                        output: Some(Register::AX),
                    })
                    .into(),
                );
                instrs.push(
                    Instruction::JumpIf(JumpIf {
                        if_true: false,
                        value: Operand::Register(Register::AX),
                        pos: JumpPos::Label(done.clone()),
                    })
                    .into(),
                );
                let default = self.emit_operand(default, &mut instrs)?;
                instrs.push(self.set_instr(Operand::Var(tmp.clone()), default).into());
                instrs.push(LabelledInstruction {
                    label: Some(done),
                    instruction: Instruction::Noop,
                });
                instrs.extend(self.bind_pattern(target, Operand::Var(tmp))?);
            }
            Pattern::Array(elements) => {
                // The value is read once, into a temporary, so an
                // initializer like `q.reverse()` isn't evaluated per element.
                let source = self.new_temp();
                instrs.push(
                    Instruction::Iterable(Iterable {
                        value,
                        output: Operand::Register(Register::AX),
                        pc: None,
                    })
                    .into(),
                );
                instrs.push(Self::declare_instr(
                    source.clone(),
                    VariableType::Any,
                    Operand::Register(Register::AX),
                ));
                for (i, element) in elements.iter().enumerate() {
                    match element {
                        // A hole: `[, b]` skips element 0.
                        None => {}
                        // `...rest` gets `source.slice(i)`.
                        Some(Pattern::Rest(target)) => {
                            instrs.push(
                                Instruction::GetAttr(GetAttr {
                                    object: Operand::Var(source.clone()),
                                    attrib: "slice".into(),
                                    output: Operand::Register(Register::AX),
                                    obj_output: Operand::Register(Register::BX),
                                })
                                .into(),
                            );
                            instrs.push(
                                Instruction::Call(Call {
                                    object: Some(Operand::Register(Register::BX)),
                                    function: Operand::Register(Register::AX),
                                    args: vec![Operand::Value((i as f64).into())],
                                })
                                .into(),
                            );
                            instrs.extend(
                                self.bind_pattern(target, Operand::Register(Register::AX))?,
                            );
                        }
                        Some(element) => {
                            instrs.push(
                                Instruction::GetIndex(GetIndex {
                                    object: Operand::Var(source.clone()),
                                    index: Operand::Value((i as f64).into()),
                                    output: Operand::Register(Register::AX),
                                })
                                .into(),
                            );
                            instrs.extend(
                                self.bind_pattern(element, Operand::Register(Register::AX))?,
                            );
                        }
                    }
                }
            }
            Pattern::Rest(_) => {
                return Err(JSError::UnknownError(
                    "a rest element can only appear inside a pattern".into(),
                ));
            }
            Pattern::Object(_) => {
                return Err(JSError::NotSupported("object destructuring".into()));
            }
        }
        Ok(instrs)
    }

    fn emit_vardecl_expr(&mut self, decl: &VarDecl) -> Result<InstructionType, JSError> {
        Ok(InstructionType::Instruction(self.emit_vardecl(decl)?))
    }

    #[allow(unused)]
    fn emit_exprs(
        &mut self,
        ex: &Expr,
        output: Option<Register>,
    ) -> Result<InstructionType, JSError> {
        if self.verbose {
            println!("In Exprs: {:?}", ex);
        }
        match ex {
            Expr::New { callee, args } => todo!(),
            Expr::Yield {
                span,
                delegate,
                value,
            } => todo!(),
            Expr::This(_token_span) => Ok(InstructionType::Value(Operand::Var("this".into()))),
            Expr::Super(_token_span) => todo!(),
            Expr::Object(_items) => todo!(),
            Expr::Function(_function) => todo!(),
            Expr::Arrow(_arrow) => todo!(),
            Expr::Class(_class) => todo!(),
            Expr::Spread(_expr) => todo!(),
            Expr::Literal(literal) => Ok(InstructionType::Value(literal.into())),
            Expr::Ident(ident) => Ok(InstructionType::Value(Operand::Var(ident.value.clone()))),
            Expr::Paren(expr) => Ok(self.emit_exprs(expr, output)?),
            Expr::Array(exprs) => Ok(self.emit_array_expr(exprs)?),
            Expr::Unary { op, value } => Ok(self.emit_unary_expr(op, value)?),
            Expr::Update { op, prefix, target } => Ok(self.emit_update_expr(op, *prefix, target)?),
            Expr::Binary { left, op, right } => Ok(self.emit_binary_expr(left, op, right, output)?),
            Expr::Conditional {
                test,
                then,
                otherwise,
            } => Ok(self.emit_conditional_expr(test, then, otherwise)?),
            Expr::Assign { target, op, value } => Ok(self.emit_assign_expr(target, op, value)?),
            Expr::Sequence(exprs) => Ok(self.emit_sequence_expr(exprs)?),
            Expr::Member {
                object,
                property,
                optional,
            } => Ok(self.emit_member_expr(object, property, *optional)?),
            Expr::Index {
                object,
                index,
                optional,
            } => Ok(self.emit_index_expr(object, index, *optional)?),
            Expr::Call {
                callee,
                args,
                optional,
            } => Ok(self.emit_call_expr(callee, args, *optional)?),
        }
    }

    fn emit_array_expr(&mut self, array: &[Option<Expr>]) -> Result<InstructionType, JSError> {
        let mut instr = vec![];
        for a in array {
            if let Some(val) = a {
                let ins = self.emit_exprs(val, None)?;
                match ins {
                    InstructionType::Instruction(i) => {
                        instr.extend(i);
                        instr.push(
                            Instruction::Push(super::instr::Push {
                                value: Operand::Register(Register::AX),
                            })
                            .into(),
                        );
                    }
                    InstructionType::Value(v) => {
                        instr.push(Instruction::Push(Push { value: v }).into());
                    }
                }
            } else {
                instr.push(
                    Instruction::Push(Push {
                        value: Operand::Value(JsValue::Null),
                    })
                    .into(),
                );
            }
        }
        instr.push(
            Instruction::GetAttr(GetAttr {
                object: Operand::Var("Array".into()),
                attrib: "new".into(),
                output: Operand::Register(Register::AX),
                obj_output: Operand::Register(Register::BX),
            })
            .into(),
        );
        instr.push(
            Instruction::Call(Call {
                object: Some(Operand::Register(Register::BX)),
                function: Operand::Register(Register::AX),
                args: vec![array.len().into()],
            })
            .into(),
        );
        Ok(InstructionType::Instruction(instr))
    }

    fn emit_block(&mut self, block: &[Statement]) -> Result<Vec<LabelledInstruction>, JSError> {
        let mut declarations = hoist_declarations(block, false);
        declarations.extend(self.emit_function_declarations(block)?);
        // Only a block that declares something needs a scope of its own;
        // otherwise its statements run in the enclosing scope. Temporaries
        // then land in the enclosing scope, which is harmless since they're
        // reused statement to statement.
        let own_scope = !declarations.is_empty();
        let mut ins = vec![];
        if own_scope {
            ins.push(Instruction::BeginScope.into());
            ins.extend(declarations);
            self.scope_depth += 1;
        }
        for stmt in block {
            ins.extend(self.emit_statement(stmt)?);
        }
        if own_scope {
            ins.push(Instruction::EndScope.into());
            self.scope_depth -= 1;
        }
        Ok(ins)
    }

    fn emit_if(
        &mut self,
        test: &Expr,
        then: &Statement,
        otherwise: &Option<Box<Statement>>,
    ) -> Result<Vec<LabelledInstruction>, JSError> {
        let mut ins = vec![];
        let test = self.emit_exprs(test, None)?;
        let test_label = self.new_label();
        match test {
            InstructionType::Instruction(i) => {
                ins.extend(i);
                ins.push(
                    Instruction::JumpIf(JumpIf {
                        if_true: true,
                        value: Operand::Register(Register::AX),
                        pos: JumpPos::Label(test_label.clone()),
                    })
                    .into(),
                );
            }
            InstructionType::Value(v) => {
                ins.push(
                    Instruction::JumpIf(JumpIf {
                        if_true: true,
                        value: v,
                        pos: JumpPos::Label(test_label.clone()),
                    })
                    .into(),
                );
            }
        }
        let end_label = self.new_label();
        if let Some(otherwise) = otherwise {
            ins.extend(self.emit_statement(otherwise)?);
        }
        ins.push(
            Instruction::Jump(Jump {
                pos: JumpPos::Label(end_label.clone()),
            })
            .into(),
        );
        ins.push(LabelledInstruction {
            label: Some(test_label),
            instruction: Instruction::Noop,
        });
        ins.extend(self.emit_statement(then)?);

        ins.push(LabelledInstruction {
            label: Some(end_label),
            instruction: Instruction::Noop,
        });
        Ok(ins)
    }

    fn emit_conditional_expr(
        &mut self,
        test: &Expr,
        then: &Expr,
        otherwise: &Expr,
    ) -> Result<InstructionType, JSError> {
        let mut ins = vec![];
        let test = self.emit_exprs(test, None)?;
        let then = self.emit_exprs(then, None)?;
        let otherwise = self.emit_exprs(otherwise, None)?;
        let test_label = match test {
            InstructionType::Instruction(i) => {
                let label = self.new_label();
                ins.extend(i);
                ins.push(
                    Instruction::JumpIf(JumpIf {
                        if_true: true,
                        value: Register::AX.into(),
                        pos: JumpPos::Label(label.clone()),
                    })
                    .into(),
                );
                label
            }
            InstructionType::Value(v) => {
                let label = self.new_label();
                ins.push(
                    Instruction::JumpIf(JumpIf {
                        if_true: true,
                        value: v,
                        pos: JumpPos::Label(label.clone()),
                    })
                    .into(),
                );
                label
            }
        };
        let end_label = self.new_label();
        match otherwise {
            InstructionType::Instruction(i) => {
                ins.extend(i);
            }
            InstructionType::Value(v) => {
                ins.push(self.set_instr(Operand::Register(Register::AX), v).into());
            }
        }
        ins.push(
            Instruction::Jump(Jump {
                pos: JumpPos::Label(end_label.clone()),
            })
            .into(),
        );
        ins.push(LabelledInstruction {
            label: Some(test_label),
            instruction: Instruction::Noop,
        });
        match then {
            InstructionType::Instruction(i) => {
                ins.extend(i);
                ins.push(
                    Instruction::Jump(Jump {
                        pos: JumpPos::Label(end_label.clone()),
                    })
                    .into(),
                );
            }
            InstructionType::Value(v) => {
                ins.push(self.set_instr(Operand::Register(Register::AX), v).into());
                ins.push(
                    Instruction::Jump(Jump {
                        pos: JumpPos::Label(end_label.clone()),
                    })
                    .into(),
                );
            }
        };

        ins.push(LabelledInstruction {
            label: Some(end_label),
            instruction: Instruction::Noop,
        });
        Ok(InstructionType::Instruction(ins))
    }

    /// `object[index]`.
    fn emit_index_expr(
        &mut self,
        object: &Expr,
        index: &Expr,
        optional: bool,
    ) -> Result<InstructionType, JSError> {
        if optional {
            return Err(JSError::NotSupported("optional indexing (`a?.[i]`)".into()));
        }
        let mut ins = vec![];
        let object_op = self.emit_operand(object, &mut ins)?;
        let mut index_ins = vec![];
        let index_op = self.emit_operand(index, &mut index_ins)?;
        let object_op = self.keep(object_op, &index_ins, index, &mut ins);
        ins.extend(index_ins);
        ins.push(
            Instruction::GetIndex(GetIndex {
                object: object_op,
                index: index_op,
                output: Operand::Register(Register::AX),
            })
            .into(),
        );
        Ok(InstructionType::Instruction(ins))
    }

    /// `object[index] = value`, or a compound form like `object[index] += value`.
    /// Like JS, it evaluates the object, then the index, then (for compound
    /// forms) reads the current element, then evaluates the value.
    fn emit_index_assign(
        &mut self,
        object: &Expr,
        index: &Expr,
        optional: bool,
        op: &AssignOp,
        value: &Expr,
    ) -> Result<InstructionType, JSError> {
        if optional {
            return Err(JSError::NotSupported(
                "assigning through optional chaining".into(),
            ));
        }
        let mut ins = vec![];
        let object_op = self.emit_operand(object, &mut ins)?;
        let mut index_ins = vec![];
        let index_op = self.emit_operand(index, &mut index_ins)?;
        let mut object_op = self.keep(object_op, &index_ins, index, &mut ins);
        ins.extend(index_ins);
        let mut index_op = index_op;

        let compound = compound_op(*op);
        let mut current = None;
        if compound.is_some() {
            // Reading the current element overwrites AX.
            object_op = self.keep_register(object_op, &mut ins);
            index_op = self.keep_register(index_op, &mut ins);
            ins.push(
                Instruction::GetIndex(GetIndex {
                    object: object_op.clone(),
                    index: index_op.clone(),
                    output: Operand::Register(Register::AX),
                })
                .into(),
            );
            current = Some(Operand::Register(Register::AX));
        }

        let mut value_ins = vec![];
        let value_op = self.emit_operand(value, &mut value_ins)?;
        let object_op = self.keep(object_op, &value_ins, value, &mut ins);
        let index_op = self.keep(index_op, &value_ins, value, &mut ins);
        let current = current.map(|c| self.keep(c, &value_ins, value, &mut ins));
        ins.extend(value_ins);

        let result = match (compound, current) {
            (Some(bin), Some(current)) => {
                ins.push(
                    Instruction::Binary(Binary {
                        left: current,
                        op: bin,
                        right: value_op,
                        output: Some(Register::AX),
                    })
                    .into(),
                );
                Operand::Register(Register::AX)
            }
            _ => value_op,
        };
        ins.push(
            Instruction::SetIndex(SetIndex {
                object: object_op,
                index: index_op,
                value: result.clone(),
            })
            .into(),
        );
        // The assignment's own value, like any instruction list's result,
        // lives in AX. Set it only after storing: the object or index may be
        // in AX until then.
        if !matches!(result, Operand::Register(Register::AX)) {
            ins.push(
                self.set_instr(Operand::Register(Register::AX), result)
                    .into(),
            );
        }
        Ok(InstructionType::Instruction(ins))
    }

    /// Protects an operand computed earlier from code that runs after it:
    /// `later`, the instructions emitted for `expr`. Those would overwrite a
    /// register, and could reassign a variable if `expr` has side effects,
    /// so in either case the value is copied into a temporary first.
    fn keep(
        &mut self,
        operand: Operand,
        later: &[LabelledInstruction],
        expr: &Expr,
        ins: &mut Vec<LabelledInstruction>,
    ) -> Operand {
        let copy = match &operand {
            Operand::Register(_) => !later.is_empty(),
            Operand::Var(_) => may_have_effects(expr),
            Operand::Value(_) => false,
        };
        if copy {
            self.copy_to_temp(operand, ins)
        } else {
            operand
        }
    }

    /// Copies a register operand into a temporary; other operands are kept.
    fn keep_register(&mut self, operand: Operand, ins: &mut Vec<LabelledInstruction>) -> Operand {
        match operand {
            Operand::Register(_) => self.copy_to_temp(operand, ins),
            other => other,
        }
    }

    fn copy_to_temp(&mut self, operand: Operand, ins: &mut Vec<LabelledInstruction>) -> Operand {
        let tmp = self.new_temp();
        ins.push(Self::declare_instr(tmp.clone(), VariableType::Any, operand));
        Operand::Var(tmp)
    }

    fn emit_assign_expr(
        &mut self,
        target: &Expr,
        op: &AssignOp,
        value: &Expr,
    ) -> Result<InstructionType, JSError> {
        match target {
            Expr::Index {
                object,
                index,
                optional,
            } => return self.emit_index_assign(object, index, *optional, op, value),
            Expr::Paren(inner) => return self.emit_assign_expr(inner, op, value),
            _ => {}
        }
        let ident = self.emit_exprs(target, None)?;
        if let InstructionType::Value(Operand::Var(v)) = ident {
            let (mut ins, rv) = match self.emit_exprs(value, None)? {
                InstructionType::Value(rv) => (vec![], rv),
                InstructionType::Instruction(ins) => (ins, Operand::Register(Register::AX)),
            };
            // `x op= y` is `x = x op y`.
            let rv = match compound_op(*op) {
                None => rv,
                Some(bin) => {
                    ins.push(
                        Instruction::Binary(Binary {
                            left: Operand::Var(v.clone()),
                            op: bin,
                            right: rv,
                            output: Some(Register::AX),
                        })
                        .into(),
                    );
                    Operand::Register(Register::AX)
                }
            };
            // An assignment is an expression whose value is the assigned
            // value, and an instruction list's result lives in AX.
            if !matches!(rv, Operand::Register(Register::AX)) {
                ins.push(
                    self.set_instr(Operand::Register(Register::AX), rv.clone())
                        .into(),
                );
            }
            ins.push(
                Instruction::Set(Set {
                    l_value: Operand::Var(v),
                    r_value: rv,
                })
                .into(),
            );
            Ok(InstructionType::Instruction(ins))
        } else {
            todo!()
        }
    }

    fn emit_binary_expr(
        &mut self,
        left: &Expr,
        op: &BinaryOp,
        right: &Expr,
        output: Option<Register>,
    ) -> Result<InstructionType, JSError> {
        let left = self.emit_exprs(left, None)?;
        let right = self.emit_exprs(right, None)?;
        let mut ins = vec![];
        // An instruction list leaves its result in AX.
        let mut left = match left {
            InstructionType::Value(v) => v,
            InstructionType::Instruction(l) => {
                ins.extend(l);
                Operand::Register(Register::AX)
            }
        };
        let right = match right {
            InstructionType::Value(v) => v,
            InstructionType::Instruction(r) => {
                // The right side's instructions reuse AX, so move the
                // left result out of the way first.
                if let Operand::Register(reg) = left {
                    let tmp = self.new_temp();
                    ins.push(Self::declare_instr(
                        tmp.clone(),
                        VariableType::Any,
                        Operand::Register(reg),
                    ));
                    left = Operand::Var(tmp);
                }
                ins.extend(r);
                Operand::Register(Register::AX)
            }
        };
        ins.push(
            Instruction::Binary(Binary {
                left,
                op: *op,
                right,
                output,
            })
            .into(),
        );
        Ok(InstructionType::Instruction(ins))
    }

    /// `op value`. With a constant operand (`-1`, `!true`) the result is
    /// computed now and no instruction is emitted.
    fn emit_unary_expr(&mut self, op: &UnaryOp, value: &Expr) -> Result<InstructionType, JSError> {
        let op = match op {
            UnaryOp::Plus => UnaryOperator::Plus,
            UnaryOp::Minus => UnaryOperator::Minus,
            UnaryOp::Not => UnaryOperator::Not,
            UnaryOp::BitNot => UnaryOperator::BitNot,
            UnaryOp::TypeOf => UnaryOperator::TypeOf,
            UnaryOp::Void => {
                // `void x` evaluates `x` only for its effects.
                let mut ins = vec![];
                self.emit_operand(value, &mut ins)?;
                ins.push(
                    self.set_instr(
                        Operand::Register(Register::AX),
                        Operand::Value(JsValue::Undefined),
                    )
                    .into(),
                );
                return Ok(InstructionType::Instruction(ins));
            }
            UnaryOp::Delete => return Err(JSError::NotSupported("delete".into())),
            UnaryOp::Await => {
                return Err(JSError::NotSupported("await (async functions)".into()));
            }
        };
        let mut ins = vec![];
        let value = self.emit_operand(value, &mut ins)?;
        if let Operand::Value(v) = &value {
            // Errors (`+1n`) are left for run time.
            if let Ok(result) = op.apply(v) {
                return Ok(InstructionType::Value(Operand::Value(result)));
            }
        }
        ins.push(Self::unary_instr(
            op,
            value,
            Operand::Register(Register::AX),
        ));
        Ok(InstructionType::Instruction(ins))
    }

    /// `++x`, `x++`, `--x`, `x--`. The stored value is the old one as a
    /// Number or BigInt, plus or minus one. The expression's value is the new
    /// value for the prefix form, and the old one (as a number) for the
    /// postfix form.
    fn emit_update_expr(
        &mut self,
        op: &UpdateOp,
        prefix: bool,
        target: &Expr,
    ) -> Result<InstructionType, JSError> {
        let step = match op {
            UpdateOp::Increment => UnaryOperator::Increment,
            UpdateOp::Decrement => UnaryOperator::Decrement,
        };
        let ax = Operand::Register(Register::AX);
        match target {
            Expr::Paren(inner) => self.emit_update_expr(op, prefix, inner),
            Expr::Ident(ident) => {
                let var = Operand::Var(ident.value.clone());
                let ins = if prefix {
                    vec![
                        Self::unary_instr(step, var.clone(), ax.clone()),
                        self.set_instr(var, ax).into(),
                    ]
                } else {
                    vec![
                        Self::unary_instr(UnaryOperator::ToNumeric, var.clone(), ax.clone()),
                        Self::unary_instr(step, ax, var),
                    ]
                };
                Ok(InstructionType::Instruction(ins))
            }
            Expr::Index {
                object,
                index,
                optional,
            } => {
                if *optional {
                    return Err(JSError::NotSupported(
                        "updating through optional chaining".into(),
                    ));
                }
                let mut ins = vec![];
                let object_op = self.emit_operand(object, &mut ins)?;
                let mut index_ins = vec![];
                let index_op = self.emit_operand(index, &mut index_ins)?;
                let object_op = self.keep(object_op, &index_ins, index, &mut ins);
                ins.extend(index_ins);
                // Reading the element overwrites AX.
                let object_op = self.keep_register(object_op, &mut ins);
                let index_op = self.keep_register(index_op, &mut ins);
                ins.push(
                    Instruction::GetIndex(GetIndex {
                        object: object_op.clone(),
                        index: index_op.clone(),
                        output: ax.clone(),
                    })
                    .into(),
                );
                // The prefix form's new value is its result, in AX. The
                // postfix form keeps the old value in AX and the new one in BX.
                let new_value = if prefix {
                    ins.push(Self::unary_instr(step, ax.clone(), ax.clone()));
                    ax
                } else {
                    let bx = Operand::Register(Register::BX);
                    ins.push(Self::unary_instr(
                        UnaryOperator::ToNumeric,
                        ax.clone(),
                        ax.clone(),
                    ));
                    ins.push(Self::unary_instr(step, ax, bx.clone()));
                    bx
                };
                ins.push(
                    Instruction::SetIndex(SetIndex {
                        object: object_op,
                        index: index_op,
                        value: new_value,
                    })
                    .into(),
                );
                Ok(InstructionType::Instruction(ins))
            }
            Expr::Member { .. } => Err(JSError::NotSupported("updating a property".into())),
            _ => Err(JSError::BadJavascript(vec![
                "Invalid left-hand side expression in update operation".into(),
            ])),
        }
    }

    fn unary_instr(op: UnaryOperator, value: Operand, output: Operand) -> LabelledInstruction {
        Instruction::Unary(Unary { op, value, output }).into()
    }

    /// `a, b, c`: each in turn; the value is the last one's.
    fn emit_sequence_expr(&mut self, exprs: &[Expr]) -> Result<InstructionType, JSError> {
        let Some((last, rest)) = exprs.split_last() else {
            return Err(JSError::UnknownError("empty sequence expression".into()));
        };
        let mut ins = vec![];
        for ex in rest {
            self.emit_operand(ex, &mut ins)?;
        }
        let last = self.emit_operand(last, &mut ins)?;
        if ins.is_empty() {
            return Ok(InstructionType::Value(last));
        }
        if !matches!(last, Operand::Register(Register::AX)) {
            ins.push(self.set_instr(Operand::Register(Register::AX), last).into());
        }
        Ok(InstructionType::Instruction(ins))
    }

    fn emit_member_expr(
        &mut self,
        object: &Expr,
        prop: &Ident,
        _optional: bool,
    ) -> Result<InstructionType, JSError> {
        // The object may itself need computing (`[1, 2].length`,
        // `a.slice(1).join()`), leaving it in AX.
        let (mut ins, object) = match self.emit_exprs(object, None)? {
            InstructionType::Value(v) => (vec![], v),
            InstructionType::Instruction(i) => (i, Operand::Register(Register::AX)),
        };
        ins.push(
            Instruction::GetAttr(GetAttr {
                object,
                attrib: prop.value.as_str().into(),
                output: Operand::Register(Register::AX),
                obj_output: Operand::Register(Register::BX),
            })
            .into(),
        );
        Ok(InstructionType::Instruction(ins))
    }

    fn emit_call_expr(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        _optional: bool,
    ) -> Result<InstructionType, JSError> {
        let mut names = vec![];
        let mut ins = vec![];
        for arg in args {
            let name = self.new_temp();
            names.push(Operand::Var(name.clone()));
            // An instruction list leaves its result in AX. The value is copied
            // into the temp right away, before the next argument reuses AX.
            let value = match self.emit_exprs(arg, None)? {
                InstructionType::Instruction(i) => {
                    ins.extend(i);
                    Operand::Register(Register::AX)
                }
                InstructionType::Value(v) => v,
            };
            ins.push(Self::declare_instr(name, VariableType::Any, value));
        }
        // Only a member expression (`obj.f`) leaves an object in BX, to
        // become the call's `this`.
        let is_method = matches!(unparen(callee), Expr::Member { .. });
        let callee = self.emit_exprs(callee, None)?;
        match callee {
            InstructionType::Instruction(i) => {
                ins.extend(i);
                ins.push(
                    Instruction::Call(Call {
                        object: is_method.then_some(Operand::Register(Register::BX)),
                        args: names,
                        function: Operand::Register(Register::AX),
                    })
                    .into(),
                );
            }
            InstructionType::Value(v) => {
                ins.push(
                    Instruction::Call(Call {
                        object: None,
                        function: v,
                        args: names,
                    })
                    .into(),
                );
            }
        }
        Ok(InstructionType::Instruction(ins))
    }

    pub fn set_instr(&self, l_value: Operand, r_value: Operand) -> Instruction {
        Instruction::Set(Set { l_value, r_value })
    }

    pub fn set_register(&mut self, register: Register, value: JsValue) {
        self.registers[register as usize] = value;
    }

    pub fn get_register(&self, register: Register) -> JsValue {
        self.registers[register as usize].clone()
    }

    pub fn resolve_value(&self, value: &Operand, scope: &Rc<Scope>) -> Result<JsValue, JSError> {
        let value = match value {
            Operand::Value(p) => p.clone(),
            Operand::Var(v) => scope.get_variable(v)?,
            Operand::Register(r) => self.get_register(*r),
        };
        Ok(value)
    }

    /// A fresh temporary's name. The caller emits a `Declare` for it, so it's
    /// created in whichever scope is current when the code runs.
    fn new_temp(&mut self) -> String {
        let name = format!("__t{}", self.temps);
        self.temps += 1;
        name
    }

    fn declare_instr(name: String, kind: VariableType, value: Operand) -> LabelledInstruction {
        Instruction::Declare(Declare { name, kind, value }).into()
    }

    pub fn new_label(&mut self) -> String {
        let label = format!("__L{}", self.labels);
        self.labels += 1;
        label
    }
}

/// Resolves jump labels to instruction positions and removes the label
/// markers, leaving plain instructions that can run on any engine.
fn link(instrs: Vec<LabelledInstruction>) -> Result<Vec<Instruction>, JSError> {
    let mut targets = HashMap::new();
    let mut code = Vec::with_capacity(instrs.len());
    for LabelledInstruction { label, instruction } in instrs {
        if let Some(label) = label {
            // A label marks the position of the next instruction kept.
            targets.insert(label, code.len());
            if matches!(instruction, Instruction::Noop) {
                continue;
            }
        }
        code.push(instruction);
    }
    for instruction in &mut code {
        // Relative jumps would need adjusting for the removed markers;
        // the compiler doesn't emit any.
        if let Instruction::Jump(Jump { pos }) | Instruction::JumpIf(JumpIf { pos, .. }) =
            instruction
            && let JumpPos::Label(label) = pos
        {
            let target = targets
                .get(label.as_str())
                .ok_or_else(|| JSError::UnknownError(format!("unknown label {label}")))?;
            *pos = JumpPos::Global(*target);
        }
    }
    Ok(code)
}

/// `Hoist` instructions for the names a scope declares: its own `let` and
/// `const`, plus at function level (for now, the whole program) every `var`
/// inside it, including in nested blocks.
fn hoist_declarations(stmts: &[Statement], function_level: bool) -> Vec<LabelledInstruction> {
    let mut names: Vec<(String, VariableType)> = vec![];
    for st in stmts {
        if let Statement::VarDecl(decl) = st
            && !matches!(decl.kind, DeclKind::Var(_))
        {
            let kind = VariableType::from(decl.kind.clone());
            for d in &decl.decls {
                names.extend(
                    binding_names(&d.target)
                        .into_iter()
                        .map(|n| (n, kind.clone())),
                );
            }
        }
        if let Statement::Function(func) = st
            && let Some(name) = &func.name
        {
            let kind = if function_level {
                VariableType::Any
            } else {
                VariableType::Let
            };
            names.push((name.value.clone(), kind));
        }
    }
    if function_level {
        let mut vars = vec![];
        for st in stmts {
            collect_vars(st, &mut vars);
        }
        names.extend(vars.into_iter().map(|n| (n, VariableType::Any)));
    }
    let mut seen = HashSet::new();
    names
        .into_iter()
        .filter(|(name, _)| seen.insert(name.clone()))
        .map(|(name, kind)| Instruction::Hoist(Hoist { name, kind }).into())
        .collect()
}

/// `var` names declared anywhere in `st`, through nested blocks and
/// statement bodies (but not into functions, which have their own scope).
fn collect_vars(st: &Statement, out: &mut Vec<String>) {
    let decl = |d: &VarDecl, out: &mut Vec<String>| {
        if matches!(d.kind, DeclKind::Var(_)) {
            out.extend(d.decls.iter().flat_map(|d| binding_names(&d.target)));
        }
    };
    match st {
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
                out.extend(binding_names(target));
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
            for s in block
                .iter()
                .chain(handler)
                .chain(finalizer.iter().flatten())
            {
                collect_vars(s, out);
            }
        }
        Statement::Switch { cases, .. } => {
            for s in cases.iter().flat_map(|c| &c.body) {
                collect_vars(s, out);
            }
        }
        _ => {}
    }
}

/// The variable a declaration binds. Destructuring isn't supported by the
/// emitter yet.
/// Every variable a declaration target binds: `x` binds x, and
/// `[a, [b = 1], ...c]` binds a, b and c.
fn binding_names(target: &Pattern) -> Vec<String> {
    fn walk(pattern: &Pattern, out: &mut Vec<String>) {
        match pattern {
            Pattern::Ident(id) => out.push(id.value.clone()),
            Pattern::Array(elements) => elements.iter().flatten().for_each(|e| walk(e, out)),
            Pattern::Default(target, _) | Pattern::Rest(target) => walk(target, out),
            Pattern::Object(props) => {
                for prop in props {
                    match prop {
                        PatternProp::KeyValue(_, p) | PatternProp::Rest(p) => walk(p, out),
                        PatternProp::Shorthand(id, _) => out.push(id.value.clone()),
                    }
                }
            }
        }
    }
    let mut out = vec![];
    walk(target, &mut out);
    out
}

/// Whether evaluating `ex` could change a variable or an object (by
/// assigning, calling something, ...). Errs on the side of yes.
/// `ex` without any parentheses around it.
fn unparen(mut ex: &Expr) -> &Expr {
    while let Expr::Paren(inner) = ex {
        ex = inner;
    }
    ex
}

fn may_have_effects(ex: &Expr) -> bool {
    match ex {
        Expr::Literal(_) | Expr::Ident(_) | Expr::This(_) | Expr::Super(_) => false,
        Expr::Paren(e) | Expr::Spread(e) => may_have_effects(e),
        Expr::Unary { op, value } => {
            matches!(op, UnaryOp::Delete | UnaryOp::Await) || may_have_effects(value)
        }
        Expr::Binary { left, right, .. } => may_have_effects(left) || may_have_effects(right),
        Expr::Conditional {
            test,
            then,
            otherwise,
        } => may_have_effects(test) || may_have_effects(then) || may_have_effects(otherwise),
        Expr::Sequence(es) => es.iter().any(may_have_effects),
        Expr::Array(elements) => elements.iter().flatten().any(may_have_effects),
        Expr::Member { object, .. } => may_have_effects(object),
        Expr::Index { object, index, .. } => may_have_effects(object) || may_have_effects(index),
        _ => true,
    }
}

/// The binary operator a compound assignment applies, or `None` for plain
/// `=`.
///
/// `&&=`, `||=` and `??=` are emitted as `x = x && y` etc. That's the same
/// result, except that JS skips evaluating `y` and the assignment entirely
/// when `x` already decides the outcome (observable via side effects in `y`,
/// or `const x`).
fn compound_op(op: AssignOp) -> Option<BinaryOp> {
    Some(match op {
        AssignOp::Assign => return None,
        AssignOp::Add => BinaryOp::Add,
        AssignOp::Sub => BinaryOp::Sub,
        AssignOp::Mul => BinaryOp::Mul,
        AssignOp::Div => BinaryOp::Div,
        AssignOp::Mod => BinaryOp::Mod,
        AssignOp::Exp => BinaryOp::Exp,
        AssignOp::Shl => BinaryOp::Shl,
        AssignOp::Shr => BinaryOp::Shr,
        AssignOp::UShr => BinaryOp::UShr,
        AssignOp::BitAnd => BinaryOp::BitAnd,
        AssignOp::BitOr => BinaryOp::BitOr,
        AssignOp::BitXor => BinaryOp::BitXor,
        AssignOp::And => BinaryOp::And,
        AssignOp::Or => BinaryOp::Or,
        AssignOp::Nullish => BinaryOp::Nullish,
    })
}

impl std::fmt::Display for JSEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "registers: {:?}", self.registers)?;
        write!(f, "{}", self.scope)
    }
}

impl From<JsValue> for Option<Operand> {
    fn from(value: JsValue) -> Self {
        Some(Operand::Value(value))
    }
}

impl From<&Literal> for Operand {
    fn from(value: &Literal) -> Self {
        Self::Value(value.into())
    }
}

impl From<JsValue> for Operand {
    fn from(value: JsValue) -> Self {
        Operand::Value(value)
    }
}

impl From<f64> for Operand {
    fn from(value: f64) -> Self {
        Self::Value(JsValue::from(value))
    }
}

impl From<Instruction> for LabelledInstruction {
    fn from(value: Instruction) -> Self {
        Self {
            label: None,
            instruction: value,
        }
    }
}

impl AsRef<Instruction> for LabelledInstruction {
    fn as_ref(&self) -> &Instruction {
        &self.instruction
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    const SOURCE: &str = "function fi() { return 'abc';} for (let a of fi()) {console.log(a);}";

    #[ignore]
    #[test]
    fn test_engine() {
        let mut engine = JSEngine::new();
        engine.exec(SOURCE).unwrap();
    }

    /// Run `src` and return the value of variable `name` afterwards.
    fn run(src: &str, name: &str) -> JsValue {
        let mut engine = JSEngine::new();
        engine.exec(src).unwrap();
        engine
            .scope
            .get_variable(name)
            .unwrap_or_else(|_| panic!("`{name}` has no value after: {src}"))
    }

    #[test]
    fn test_assign_ops() {
        for (src, expected) in [
            ("let x = 10; x = 3", JsValue::from(3.0)),
            ("let x = 10; x += 3", 13.0.into()),
            ("let x = 'a'; x += 'b'", "ab".into()),
            ("let x = 10; x -= 3", 7.0.into()),
            ("let x = 10; x *= 3", 30.0.into()),
            ("let x = 10; x /= 4", 2.5.into()),
            ("let x = 10; x %= 4", 2.0.into()),
            ("let x = 2; x **= 10", 1024.0.into()),
            ("let x = 3; x <<= 2", 12.0.into()),
            ("let x = 0 - 12; x >>= 2", (-3.0).into()),
            ("let x = 0 - 1; x >>>= 28", 15.0.into()),
            ("let x = 12; x &= 10", 8.0.into()),
            ("let x = 12; x |= 3", 15.0.into()),
            ("let x = 12; x ^= 10", 6.0.into()),
            ("let x = 1; x &&= 'set'", "set".into()),
            ("let x = 0; x &&= 'set'", 0.0.into()),
            ("let x = 0; x ||= 'set'", "set".into()),
            ("let x = 1; x ||= 'set'", 1.0.into()),
            ("let x = null; x ??= 'set'", "set".into()),
            ("let x = 0; x ??= 'set'", 0.0.into()),
        ] {
            assert_eq!(run(src, "x"), expected, "{src}");
        }
    }

    #[test]
    fn test_assign_ops_with_expressions() {
        assert_eq!(run("let x = 1; let y = 2; x += y * 3", "x"), 7.0.into());
        assert_eq!(
            run("let x = 100; let y = 2; x -= y ** 3 + 2", "x"),
            90.0.into()
        );
        assert_eq!(
            run("let x = 1; let y = 2; x += y; x *= y; x -= 1", "x"),
            5.0.into()
        );
        assert_eq!(run("let x = 5; x = x * x - 1", "x"), 24.0.into());
    }

    #[test]
    fn test_binary_ops_in_expressions() {
        for (src, expected) in [
            ("let r = 10 - 2 * 3", JsValue::from(4.0)),
            ("let r = 10 - 2 - 3", 5.0.into()),
            ("let r = 2 ** 3 ** 2", 512.0.into()),
            (
                "let a = 1; let c = 2; let d = 3; let r = a - c * d * d",
                (-17.0).into(),
            ),
            ("let a = 2; let b = 3; let r = a * b + b * a", 12.0.into()),
            ("let a = 7; let r = a % 4 + a / 2", 6.5.into()),
            ("let r = 1 + 2 < 4", true.into()),
            ("let r = 1 < 2 == 2 > 1", true.into()),
            ("let r = '1' == 1", true.into()),
            ("let r = '1' === 1", false.into()),
            ("let r = 1 != 2", true.into()),
            ("let r = 1 !== 1", false.into()),
            ("let r = 5 & 3 | 8", 9.0.into()),
            ("let r = 1 << 4 >> 2", 4.0.into()),
            ("let r = 0 || 2 && 3", 3.0.into()),
            ("let r = null ?? 'fallback'", "fallback".into()),
            ("let r = 3 >= 3", true.into()),
            ("let r = 2 <= 1", false.into()),
        ] {
            assert_eq!(run(src, "r"), expected, "{src}");
        }
    }

    #[test]
    fn test_block_scoping() {
        for (src, expected) in [
            // Assigning in a block updates the outer variable.
            ("let x = 1; { x = 3; }", JsValue::from(3.0)),
            ("let x = 1; { x = 2 + 3; }", 5.0.into()),
            // A `let` in a block shadows, whether or not its initializer
            // needs instructions.
            ("let x = 1; { let x = 2; }", 1.0.into()),
            ("let x = 1; { let x = 2 + 3; }", 1.0.into()),
            ("let x = 1; { let x = 2; x = 7; }", 1.0.into()),
            // The block sees outer variables.
            (
                "let x = 1; let y = 10; { let x = y * 2; y = x + 1; } x = y",
                21.0.into(),
            ),
            ("let x = 1; { { x = x + 1; } x = x * 10; }", 20.0.into()),
        ] {
            assert_eq!(run(src, "x"), expected, "{src}");
        }
    }

    #[test]
    fn test_block_variables_do_not_leak() {
        let mut engine = JSEngine::new();
        engine.exec("{ let inner = 1 * 2 + 3 * 4; }").unwrap();
        assert!(matches!(
            engine.scope.get_variable("inner"),
            Err(JSError::ReferenceError(_))
        ));
        let globals: Vec<String> = engine.scope.vars.borrow().keys().cloned().collect();
        assert!(
            !globals.iter().any(|name| name.starts_with("__t")),
            "temporaries are declared in the block's scope, not the global one: {globals:?}"
        );
    }

    #[test]
    fn test_declarations() {
        assert_eq!(run("let x; x = 4", "x"), 4.0.into());
        assert_eq!(run("let x", "x"), JsValue::Undefined);
        assert_eq!(run("const c = 1 + 1", "c"), 2.0.into());
        assert_eq!(run("let a = 1, b = a + 1", "b"), 2.0.into());
        let mut engine = JSEngine::new();
        assert!(matches!(
            engine.exec("const c = 1; c = 2"),
            Err(JSError::InvalidReassign(_))
        ));
        let mut engine = JSEngine::new();
        assert!(matches!(
            engine.exec("undeclared = 1"),
            Err(JSError::ObjectNotFound(_))
        ));
    }

    #[test]
    fn test_if_else_picks_one_branch() {
        for (src, expected) in [
            (
                "let x = 0; if (1 < 2) { x = 1 } else { x = 2 }",
                JsValue::from(1.0),
            ),
            ("let x = 0; if (2 < 1) { x = 1 } else { x = 2 }", 2.0.into()),
            // Single-statement branches, no braces.
            ("let x = 0; if (true) x = 1; else x = 2;", 1.0.into()),
            ("let x = 0; if (false) x = 1; else x = 2;", 2.0.into()),
            // Branches with several statements.
            (
                "let x = 1; if (x == 1) { x = x + 1; x = x * 10 } else { x = 0 }",
                20.0.into(),
            ),
            (
                "let x = 1; if (x != 1) { x = 0 } else { x = x + 1; x = x * 10 }",
                20.0.into(),
            ),
            // Empty branches are fine.
            ("let x = 5; if (true) {} else { x = 0 }", 5.0.into()),
            ("let x = 5; if (false) { x = 0 } else {}", 5.0.into()),
        ] {
            assert_eq!(run(src, "x"), expected, "{src}");
        }
    }

    #[test]
    fn test_if_condition_truthiness() {
        // The condition is a plain value (no instructions) in each of these.
        for (cond, taken) in [
            ("true", true),
            ("false", false),
            ("1", true),
            ("0", false),
            ("'a'", true),
            ("''", false),
            ("null", false),
            ("c", true),
            ("z", false),
        ] {
            let src = format!("let c = 'set'; let z = 0; let x = 0; if ({cond}) x = 1; else x = 2");
            let expected = if taken { 1.0 } else { 2.0 };
            assert_eq!(run(&src, "x"), expected.into(), "{src}");
        }
        // Computed conditions, including NaN which is falsy.
        assert_eq!(
            run("let x = 0; if (0 / 0) x = 1; else x = 2", "x"),
            2.0.into()
        );
        assert_eq!(
            run("let x = 0; if (1 && 'a') x = 1; else x = 2", "x"),
            1.0.into()
        );
        assert_eq!(
            run("let x = 0; if (0 || null) x = 1; else x = 2", "x"),
            2.0.into()
        );
    }

    #[test]
    fn test_if_without_else() {
        assert_eq!(run("let x = 0; if (true) x = 1", "x"), 1.0.into());
        assert_eq!(run("let x = 0; if (1 < 2) { x = 1 }", "x"), 1.0.into());
        assert_eq!(
            run("let x = 0; if (false) x = 1", "x"),
            0.0.into(),
            "false condition must skip the branch"
        );
        assert_eq!(
            run("let x = 0; if (2 < 1) { x = 1 }", "x"),
            0.0.into(),
            "false condition must skip the branch"
        );
        // Code after the `if` runs either way.
        assert_eq!(
            run("let x = 0; if (false) x = 1; x = x + 5", "x"),
            5.0.into()
        );
        assert_eq!(
            run("let x = 0; if (true) x = 1; x = x + 5", "x"),
            6.0.into()
        );
    }

    #[test]
    fn test_else_if_chain() {
        for (n, expected) in [("0 - 5", "neg"), ("0", "zero"), ("7", "pos")] {
            let src = format!(
                "let n = {n}; let x = '';
                 if (n < 0) x = 'neg';
                 else if (n == 0) x = 'zero';
                 else x = 'pos'"
            );
            assert_eq!(run(&src, "x"), expected.into(), "n = {n}");
        }
    }

    #[test]
    fn test_nested_if() {
        let src = |a: &str, b: &str| {
            format!(
                "let a = {a}; let b = {b}; let x = 0;
                 if (a) {{ if (b) {{ x = 1 }} else {{ x = 2 }} }} else {{ if (b) {{ x = 3 }} else {{ x = 4 }} }}"
            )
        };
        assert_eq!(run(&src("true", "true"), "x"), 1.0.into());
        assert_eq!(run(&src("true", "false"), "x"), 2.0.into());
        assert_eq!(run(&src("false", "true"), "x"), 3.0.into());
        assert_eq!(run(&src("false", "false"), "x"), 4.0.into());
    }

    #[test]
    fn test_consecutive_ifs() {
        let src = "let x = 0;
                   if (true) x = x + 1; else x = x + 100;
                   if (false) x = x + 1000; else x = x + 10;
                   if (x == 11) { x = x * 2 } else { x = 0 }";
        assert_eq!(run(src, "x"), 22.0.into());
    }

    #[test]
    fn test_if_branch_blocks_are_scoped() {
        assert_eq!(run("let x = 1; if (true) { let x = 2; }", "x"), 1.0.into());
        assert_eq!(
            run("let x = 1; if (false) {} else { let x = 2; }", "x"),
            1.0.into()
        );
        assert_eq!(
            run("let x = 1; if (true) { let y = 5; x = y * 2 }", "x"),
            10.0.into()
        );
        let mut engine = JSEngine::new();
        engine
            .exec("if (true) { let inner = 1; } else { let other = 2; }")
            .unwrap();
        for name in ["inner", "other"] {
            assert!(
                matches!(
                    engine.scope.get_variable(name),
                    Err(JSError::ReferenceError(_))
                ),
                "`{name}` leaked out of its branch"
            );
        }
    }

    #[test]
    fn test_nested_blocks() {
        // Each level shadows; inner reads see the innermost binding.
        let src = "let r = 0; { let x = 2; { let x = 3; r = x; } r = r * 10 + x; }";
        assert_eq!(run(src, "r"), 32.0.into());
        // Leaving a block restores the enclosing scope.
        assert_eq!(
            run("let x = 1; { let x = 2; { let x = 3; } } x = x + 1", "x"),
            2.0.into()
        );
        assert_eq!(
            run("let x = 1; {} { x = x + 1 } {{}} x = x * 3", "x"),
            6.0.into()
        );
    }

    #[test]
    fn test_assignment_is_an_expression() {
        let mut engine = JSEngine::new();
        engine
            .exec("let x = 0; let y = (x = 5) + 1; let a = 0; let b = 0; a = b = 7; let c = 1; let d = (c += 2) * 10")
            .unwrap();
        for (name, expected) in [
            ("x", 5.0),
            ("y", 6.0),
            ("a", 7.0),
            ("b", 7.0),
            ("c", 3.0),
            ("d", 30.0),
        ] {
            assert_eq!(
                engine.scope.get_variable(name).unwrap(),
                expected.into(),
                "{name}"
            );
        }
        assert_eq!(
            run("let x = 0; if ((x = 5) > 3) x = x + 1", "x"),
            6.0.into()
        );
        assert_eq!(
            run("let x = 0; let y = 4; let z = (x = y) * 2", "z"),
            8.0.into()
        );
    }

    #[test]
    fn test_hoisting_declares_before_use() {
        // The name exists from the start of its scope, so assigning before
        // the declaration line targets it.
        assert_eq!(run("x = 5; var x;", "x"), 5.0.into());
        assert_eq!(
            run("x = 5; var x = 1;", "x"),
            1.0.into(),
            "the initializer still runs at its own line"
        );
        // Hoisting moves the declaration only, not the initializer.
        assert_eq!(run("let y = x; var x = 3;", "y"), JsValue::Undefined);
        assert_eq!(run("let y = x; var x = 3;", "x"), 3.0.into());
    }

    #[test]
    fn test_hoisting_keeps_initializers_in_order() {
        // Initializers must see the state at their own line, not at scope entry.
        assert_eq!(run("let x = 1; x = 2; let y = x", "y"), 2.0.into());
        assert_eq!(
            run("let x = 1; x = x + 1; let y = x * 10", "y"),
            20.0.into()
        );
        assert_eq!(
            run("let x = 1 + 2", "x"),
            3.0.into(),
            "computed initializer"
        );
        assert_eq!(run("let a = 1, b = a + 1", "b"), 2.0.into());
    }

    #[test]
    fn test_hoisting_in_sibling_blocks() {
        // Each block declares its own names, even at the same nesting depth.
        let src =
            "let r = 0; { let a = 1; r = r + a } { let b = 2; r = r + b } { let c = 3; r = r + c }";
        assert_eq!(run(src, "r"), 6.0.into());
        let src = "let r = 0; { { let a = 1; r = r + a } } { { let b = 20; r = r + b } }";
        assert_eq!(run(src, "r"), 21.0.into());
    }

    #[test]
    fn test_hoisting_in_if_branches() {
        let src = |cond: &str| {
            format!("let r = 0; if ({cond}) {{ let a = 1; r = a }} else {{ let b = 2; r = b }}")
        };
        assert_eq!(run(&src("true"), "r"), 1.0.into());
        assert_eq!(run(&src("false"), "r"), 2.0.into());
    }

    #[test]
    fn test_hoisted_let_shadows_from_block_start() {
        // `x` inside the block refers to the block's own `x` from its first
        // line (real JS throws here, since `let` isn't usable before its
        // declaration), so the outer `x` is never touched.
        assert_eq!(run("let x = 1; { x = 2; let x = 3; }", "x"), 1.0.into());
    }

    #[test]
    fn test_var_is_function_scoped() {
        // `var` hoists to the enclosing function (here, the top level), not
        // to the block it's written in.
        assert_eq!(run("{ var v = 1; } let r = v", "r"), 1.0.into());
    }

    #[test]
    fn test_var_redeclaration() {
        // Redeclaring a `var` without an initializer keeps its value.
        assert_eq!(run("var a = 1; var a;", "a"), 1.0.into());
        assert_eq!(run("var a = 1; var a = 2;", "a"), 2.0.into());
        // A `var` in a block is the same variable as the top-level one.
        assert_eq!(run("var a = 1; { var a = 2; }", "a"), 2.0.into());
        assert_eq!(
            run("var a = 1; if (true) { var a = a + 5; }", "a"),
            6.0.into()
        );
        // `const` is initialized exactly once, at its line.
        assert_eq!(
            run("let r = 0; { r = 1; const c = r + 1; r = c * 10 }", "r"),
            20.0.into()
        );
    }

    #[test]
    fn test_compiled_code_is_linked() {
        let src = "let x = 0; if (x < 1) { x = 1 } else { x = 2 } let y = x > 0 ? 'a' : 'b'";
        let mut compiler = JSEngine::new();
        compiler.verbose = false;
        let program = compiler.parse(src).unwrap();
        let code = compiler.compile(&program).unwrap();
        for instruction in &code {
            assert!(
                !matches!(instruction, Instruction::Noop),
                "label markers are removed"
            );
            if let Instruction::Jump(Jump { pos }) | Instruction::JumpIf(JumpIf { pos, .. }) =
                instruction
            {
                assert!(
                    matches!(pos, JumpPos::Global(target) if *target <= code.len()),
                    "jump not linked: {pos:?}"
                );
            }
        }
        // Linked code doesn't depend on the engine that compiled it.
        let mut runner = JSEngine::new();
        runner.verbose = false;
        runner.run(&code).unwrap();
        assert_eq!(runner.scope.get_variable("x").unwrap(), 1.0.into());
        assert_eq!(runner.scope.get_variable("y").unwrap(), "a".into());
    }

    #[test]
    fn test_temporaries_are_reused_across_statements() {
        // Each statement needs a temporary (a call argument, and the left side
        // of a sum whose right side is computed).
        let statement = "n = (n + a.length) + a.indexOf(n);";
        let src = format!("let a = [1, 2]; let n = 0; {}", statement.repeat(50));
        let mut engine = JSEngine::new();
        engine.verbose = false;
        engine.exec(&src).unwrap();
        let temps = engine
            .scope
            .vars
            .borrow()
            .keys()
            .filter(|name| name.starts_with("__t"))
            .count();
        assert!(temps <= 2, "{temps} temporaries in the global scope");
    }

    #[test]
    fn test_finished_scopes_are_freed() {
        let mut engine = JSEngine::new();
        engine.verbose = false;
        engine
            .exec("let x = 0; { let a = [1]; { let b = 2; x = b; } } if (true) { let c = 3; }")
            .unwrap();
        // Every variable and every child scope holds a weak reference to its
        // scope. Once execution leaves the blocks, only the global variables'
        // references should remain.
        let globals = engine.scope.vars.borrow().len();
        assert_eq!(Rc::weak_count(&engine.scope), globals);
    }

    #[test]
    fn test_for_basic() {
        let src = "let s = 0; for (let i = 0; i < 5; i = i + 1) { s = s + i }";
        assert_eq!(run(src, "s"), 10.0.into());
    }

    #[test]
    fn test_for_var_and_expression_init() {
        // `var` in the head is visible after the loop.
        let src = "let s = 0; for (var i = 0; i < 5; i = i + 1) { s = s + i }";
        assert_eq!(run(src, "s"), 10.0.into());
        assert_eq!(run(src, "i"), 5.0.into());
        // An expression as init, and a body without braces.
        let src = "let s = 0; let i; for (i = 0; i < 5; i = i + 1) s = s + i;";
        assert_eq!(run(src, "s"), 10.0.into());
        assert_eq!(run(src, "i"), 5.0.into());
    }

    #[test]
    fn test_for_false_test_skips_body() {
        let src = "let s = 7; let i; for (i = 10; i < 5; i = i + 1) { s = 0 }";
        assert_eq!(run(src, "s"), 7.0.into());
        assert_eq!(run(src, "i"), 10.0.into(), "the update doesn't run either");
    }

    #[test]
    fn test_for_optional_parts() {
        // No init and no update.
        assert_eq!(
            run("let i = 0; for (; i < 3;) { i = i + 1 }", "i"),
            3.0.into()
        );
        // A test that's a plain variable rather than a computed value.
        let src = "let n = 3; let c = 0; for (; n;) { n = n - 1; c = c + 1 }";
        assert_eq!(run(src, "c"), 3.0.into());
        assert_eq!(run(src, "n"), 0.0.into());
    }

    #[test]
    fn test_for_without_test_runs_the_body() {
        // With no `break` yet, end the infinite loop by failing in the body:
        // the error proves the body ran.
        let mut engine = JSEngine::new();
        engine.verbose = false;
        let result = engine.exec("let x = 0; for (;;) { x = 1; undeclared = 2; }");
        assert!(
            matches!(result, Err(JSError::ObjectNotFound(_))),
            "{result:?}"
        );
        assert_eq!(engine.scope.get_variable("x").unwrap(), 1.0.into());
    }

    #[test]
    fn test_for_nested() {
        let src = "let s = 0; for (let i = 0; i < 3; i = i + 1) { for (let j = 0; j < 4; j = j + 1) { s = s + 1 } }";
        assert_eq!(run(src, "s"), 12.0.into());
        let src = "let s = ''; for (let i = 0; i < 2; i = i + 1) { for (let j = 0; j < 2; j = j + 1) { s = s + i + j + ' ' } }";
        assert_eq!(run(src, "s"), "00 01 10 11 ".into());
    }

    #[test]
    fn test_for_scoping() {
        // A `let` in the body is per iteration and doesn't touch outer names.
        let src = "let x = 1; for (let i = 0; i < 3; i = i + 1) { let x = i }";
        assert_eq!(run(src, "x"), 1.0.into());
        // A `let` in the head is scoped to the loop.
        let mut engine = JSEngine::new();
        engine.verbose = false;
        engine.exec("for (let i = 0; i < 3; i = i + 1) {}").unwrap();
        assert!(matches!(
            engine.scope.get_variable("i"),
            Err(JSError::ReferenceError(_))
        ));
    }

    #[test]
    fn test_for_with_arrays_and_temporaries() {
        let src =
            "let a = []; for (let i = 0; i < 4; i = i + 1) { a.push(i * i) } let s = a.join()";
        assert_eq!(run(src, "s"), "0,1,4,9".into());
        // The test and update need temporaries of their own.
        let src = "let a = [1, 2, 3]; let s = 0; for (let i = 0; i < a.length + 0; i = i + a.indexOf(1) + 1) { s = s + a.at(i) }";
        assert_eq!(run(src, "s"), 6.0.into());
    }

    #[test]
    fn test_for_many_iterations_frees_each_iteration() {
        let mut engine = JSEngine::new();
        engine.verbose = false;
        engine
            .exec("let s = 0; for (let i = 0; i < 10000; i = i + 1) { let a = [i, i]; s = s + a.length }")
            .unwrap();
        assert_eq!(engine.scope.get_variable("s").unwrap(), 20000.0.into());
        let globals = engine.scope.vars.borrow().len();
        assert_eq!(
            Rc::weak_count(&engine.scope),
            globals,
            "the loop's scopes are freed after it finishes"
        );
        let temps = engine
            .scope
            .vars
            .borrow()
            .keys()
            .filter(|n| n.starts_with("__t"))
            .count();
        assert!(temps <= 2, "{temps} temporaries");
    }

    /// Run `src`, then read several variables.
    fn run_all(src: &str, names: &[&str]) -> Vec<JsValue> {
        let mut engine = JSEngine::new();
        engine.verbose = false;
        engine.exec(src).unwrap_or_else(|e| panic!("{src}: {e}"));
        names
            .iter()
            .map(|n| {
                engine
                    .scope
                    .get_variable(n)
                    .unwrap_or_else(|e| panic!("{n}: {e}"))
            })
            .collect()
    }

    #[test]
    fn test_array_pattern_basic() {
        assert_eq!(
            run_all("let [a, b] = [1, 2]", &["a", "b"]),
            [1.0.into(), 2.0.into()]
        );
        assert_eq!(
            run_all("let arr = [3, 4, 5]; let [x, y] = arr", &["x", "y"]),
            [3.0.into(), 4.0.into()]
        );
        assert_eq!(
            run_all("let [a, b] = [1, 2], c = a + b", &["c"]),
            [3.0.into()],
            "later declarators see earlier ones"
        );
    }

    #[test]
    fn test_array_pattern_missing_and_holes() {
        assert_eq!(
            run_all("let [a, b, c] = [1]", &["a", "b", "c"]),
            [1.0.into(), JsValue::Undefined, JsValue::Undefined]
        );
        assert_eq!(
            run_all("let [, second] = [1, 2]", &["second"]),
            [2.0.into()]
        );
        assert_eq!(
            run_all("let [a, , c] = [1, 2, 3]", &["a", "c"]),
            [1.0.into(), 3.0.into()]
        );
    }

    #[test]
    fn test_array_pattern_nested() {
        assert_eq!(
            run_all("let [a, [b, c]] = [1, [2, 3]]", &["a", "b", "c"]),
            [1.0.into(), 2.0.into(), 3.0.into()]
        );
    }

    #[test]
    fn test_array_pattern_defaults() {
        assert_eq!(
            run_all("let [a = 5, b = 6] = [1]", &["a", "b"]),
            [1.0.into(), 6.0.into()]
        );
        assert_eq!(
            run_all("let [a = 5] = [null]", &["a"]),
            [JsValue::Null],
            "a default only replaces undefined"
        );
        assert_eq!(
            run_all("let [a = 1, b = a + 1] = []", &["b"]),
            [2.0.into()],
            "defaults can use earlier elements"
        );
    }

    #[test]
    fn test_array_pattern_rest() {
        assert_eq!(
            run_all(
                "let [first, ...rest] = [1, 2, 3]; let s = rest.join()",
                &["first", "s"]
            ),
            [1.0.into(), "2,3".into()]
        );
        assert_eq!(
            run_all("let [a, ...rest] = [1]; let n = rest.length", &["n"]),
            [0.0.into()]
        );
    }

    #[test]
    fn test_array_pattern_evaluates_the_value_once() {
        // reverse() reverses in place, so evaluating it twice would undo it.
        assert_eq!(
            run_all("let q = [7, 8]; let [a, b] = q.reverse()", &["a", "b"]),
            [8.0.into(), 7.0.into()]
        );
    }

    #[test]
    fn test_array_pattern_declaration_kinds() {
        let mut engine = JSEngine::new();
        engine.verbose = false;
        assert!(matches!(
            engine.exec("const [a, b] = [1, 2]; a = 3"),
            Err(JSError::InvalidReassign(_))
        ));
        assert_eq!(
            run_all("var [a, b] = [1, 2]", &["a", "b"]),
            [1.0.into(), 2.0.into()]
        );
        assert_eq!(
            run_all("{ var [p] = [9]; } let q = p", &["q"]),
            [9.0.into()]
        );
        assert_eq!(
            run_all("let a = 1; { let [a] = [2]; }", &["a"]),
            [1.0.into()],
            "a pattern in a block shadows"
        );
    }

    #[test]
    fn test_array_pattern_in_for_head() {
        let src = "let s = 0; for (let [i, j] = [0, 10]; i < 3; i = i + 1) { s = s + j }";
        assert_eq!(run_all(src, &["s"]), [30.0.into()]);
    }

    #[test]
    fn test_array_pattern_sources() {
        assert_eq!(
            run_all("let [c1, c2] = 'hi'", &["c1", "c2"]),
            ["h".into(), "i".into()],
            "strings are iterable"
        );
        for src in ["let [a] = 5", "let [a] = null"] {
            let mut engine = JSEngine::new();
            engine.verbose = false;
            assert!(
                matches!(engine.exec(src), Err(JSError::TypeError(_))),
                "{src} should be a TypeError (not iterable)"
            );
        }
    }

    fn run_err(src: &str) -> JSError {
        let mut engine = JSEngine::new();
        engine.verbose = false;
        engine.exec(src).expect_err(src)
    }

    #[test]
    fn test_index_read() {
        let a = "let a = [10, 20, 30];";
        for (expr, expected) in [
            ("a[1]", JsValue::from(20.0)),
            ("a[0] + a[2]", 40.0.into()),
            ("a[1 + 1]", 30.0.into()),
            ("a[3]", JsValue::Undefined),
            ("a[0 - 1]", JsValue::Undefined),
            ("a[1.5]", JsValue::Undefined),
            ("a['1']", 20.0.into()),
            ("a['length']", 3.0.into()),
            ("[[1, 2], [3, 4]][1][0]", 3.0.into()),
            ("[5, 6, 7].slice(1)[0]", 6.0.into()),
            ("'hello'[1]", "e".into()),
            ("'hi'[5]", JsValue::Undefined),
            ("'abc'['length']", 3.0.into()),
        ] {
            assert_eq!(run(&format!("{a} let r = {expr}"), "r"), expected, "{expr}");
        }
        let src = "let a = [10, 20, 30]; let s = 0; for (let i = 0; i < a.length; i = i + 1) { s = s + a[i] }";
        assert_eq!(run(src, "s"), 60.0.into());
        assert!(matches!(
            run_err("let n = null; let r = n[0]"),
            JSError::TypeError(_)
        ));
    }

    #[test]
    fn test_index_write() {
        for (src, expected) in [
            ("let a = [1, 2, 3]; a[0] = 9;", "9,2,3"),
            ("let a = []; a[2] = 'x';", ",,x"),
            (
                "let a = []; for (let i = 0; i < 5; i = i + 1) { a[i] = i * i }",
                "0,1,4,9,16",
            ),
            (
                "let a = [1, 2]; let t = a[0]; a[0] = a[1]; a[1] = t;",
                "2,1",
            ),
            ("let a = [1, 2]; a[1] += 10; a[0] *= 5;", "5,12"),
            ("let a = [0, 0]; a[0] = a[1] = 4;", "4,4"),
            (
                "let a = [0, 0, 0]; let i = 1; a[i + 1] = a[i - 1] + 7;",
                "0,0,7",
            ),
            ("let a = [1, 2, 3]; a['length'] = 1;", "1"),
            ("let m = [[0]]; m[0][0] = 5; let a = m[0];", "5"),
        ] {
            assert_eq!(
                run(&format!("{src} let s = a.join()"), "s"),
                expected.into(),
                "{src}"
            );
        }
        assert_eq!(run("let a = [0]; let v = (a[0] = 7) + 1", "v"), 8.0.into());
        assert!(matches!(
            run_err("let n = null; n[0] = 1"),
            JSError::TypeError(_)
        ));
        assert!(matches!(
            run_err("let t = 'abc'; t[0] = 'x'"),
            JSError::TypeError(_)
        ));
    }

    #[test]
    fn test_index_evaluation_order() {
        // The index is evaluated before the right-hand side, even when the
        // right-hand side reassigns it.
        assert_eq!(
            run(
                "let a = [0, 0, 0]; let i = 0; a[i] = (i = 2); let s = a.join()",
                "s"
            ),
            "2,0,0".into()
        );
        // For a compound assignment, the current element is read before the
        // right-hand side runs.
        assert_eq!(
            run(
                "let a = [1, 2, 3]; let i = 0; a[i] += (i = 2); let s = a.join()",
                "s"
            ),
            "3,2,3".into()
        );
        assert_eq!(
            run(
                "let a = [1, 2]; let b = a; a[0] += (a[0] = 10); let s = b.join()",
                "s"
            ),
            "11,2".into()
        );
    }

    #[test]
    fn test_function_calls() {
        for (src, name, expected) in [
            (
                "let n = 0; function inc() { n = n + 1 } inc(); inc();",
                "n",
                JsValue::from(2.0),
            ),
            (
                "function add(a, b) { return a + b } let r = add(2, 3);",
                "r",
                5.0.into(),
            ),
            (
                "function f(a, b) { return b } let r = f(1);",
                "r",
                JsValue::Undefined,
            ),
            (
                "function f(a) { return a } let r = f(1, 2, 3);",
                "r",
                1.0.into(),
            ),
            (
                "function f() { return arguments.length } let r = f(1, 2, 3);",
                "r",
                3.0.into(),
            ),
            ("function f() {} let r = f();", "r", JsValue::Undefined),
            (
                "function f() { return; } let r = f();",
                "r",
                JsValue::Undefined,
            ),
            (
                "function f() { 1 + 2 } let r = f();",
                "r",
                JsValue::Undefined,
            ),
        ] {
            assert_eq!(run(src, name), expected, "{src}");
        }
    }

    #[test]
    fn test_return_ends_the_function() {
        let src = "let log = []; function f() { log.push(1); return 2; log.push(3); } let r = f(); let s = log.join()";
        assert_eq!(run(src, "r"), 2.0.into());
        assert_eq!(run(src, "s"), "1".into());
        let src = "function sign(x) { if (x < 0) { return 0 - 1 } if (x > 0) { return 1 } return 0 } let s = [sign(0 - 5), sign(0), sign(7)].join()";
        assert_eq!(run(src, "s"), "-1,0,1".into());
        let src = "function first(a) { for (let i = 0; i < a.length; i = i + 1) { if (a[i] > 10) { return a[i] } } return 0 - 1 } let r = first([3, 12, 40]);";
        assert_eq!(run(src, "r"), 12.0.into());
    }

    #[test]
    fn test_function_locals_and_scoping() {
        // Locals are hoisted inside the function and don't leak out.
        let src = "function f() { let x = 1; var y = 2; { let z = 3; x = x + z } return x + y } let r = f();";
        assert_eq!(run(src, "r"), 6.0.into());
        let mut engine = JSEngine::new();
        engine.verbose = false;
        engine
            .exec("function f(p) { let x = 1; var y = 2 } f(0)")
            .unwrap();
        for name in ["x", "y", "p", "arguments"] {
            assert!(
                matches!(
                    engine.scope.get_variable(name),
                    Err(JSError::ReferenceError(_))
                ),
                "`{name}` leaked out of the function"
            );
        }
        // A function sees the variables where it was defined, not where it's called.
        let src = "let x = 'global'; function f() { return x } function g() { let x = 'g'; return f() } let r = g();";
        assert_eq!(run(src, "r"), "global".into());
        // Parameters are local: assigning one doesn't touch the caller's variable.
        let src = "let a = 1; function f(a) { a = 100; return a } let r = f(a) + a;";
        assert_eq!(run(src, "r"), 101.0.into());
    }

    #[test]
    fn test_function_hoisting_and_recursion() {
        assert_eq!(
            run(
                "let r = square(4); function square(x) { return x * x }",
                "r"
            ),
            16.0.into()
        );
        let src = "function fact(n) { if (n <= 1) { return 1 } return n * fact(n - 1) } let r = fact(10);";
        assert_eq!(run(src, "r"), 3628800.0.into());
        let src = "function fib(n) { if (n < 2) { return n } return fib(n - 1) + fib(n - 2) } let r = fib(15);";
        assert_eq!(run(src, "r"), 610.0.into());
        let src = "function isEven(n) { if (n == 0) { return true } return isOdd(n - 1) } function isOdd(n) { if (n == 0) { return false } return isEven(n - 1) } let r = isEven(10);";
        assert_eq!(run(src, "r"), true.into(), "mutual recursion");
        let src = "function outer() { return inner() * 2; function inner() { return 21 } } let r = outer();";
        assert_eq!(
            run(src, "r"),
            42.0.into(),
            "nested declarations are hoisted in their function"
        );
    }

    #[test]
    fn test_function_parameters() {
        let src = "function f(a, b = a + 1) { return [a, b].join() } let r = f(1) + ' ' + f(1, 5);";
        assert_eq!(run(src, "r"), "1,2 1,5".into());
        let src = "function f(first, ...rest) { return rest.length } let r = f(1, 2, 3);";
        assert_eq!(run(src, "r"), 2.0.into());
        let src = "function f([x, y]) { return x * y } let r = f([6, 7]);";
        assert_eq!(run(src, "r"), 42.0.into());
    }

    #[test]
    fn test_functions_as_values() {
        let src = "function f() { return 7 } let g = f; let r = g();";
        assert_eq!(run(src, "r"), 7.0.into());
        let src = "function f() { return 7 } let r = f === f;";
        assert_eq!(run(src, "r"), true.into());
        let src = "function f() { return 1 } f = 5; let r = f;";
        assert_eq!(
            run(src, "r"),
            5.0.into(),
            "a declared function can be reassigned"
        );
        let src = "function twice(x) { return x * 2 } let fs = [twice]; let r = fs[0](21);";
        assert_eq!(run(src, "r"), 42.0.into());
        assert!(matches!(run_err("let x = 5; x()"), JSError::TypeError(_)));
    }

    #[test]
    fn test_calls_inside_expressions() {
        let f = "function f(x) { let t = x * 10; return t + 1 }";
        for (expr, expected) in [
            ("f(1) + f(2)", JsValue::from(32.0)),
            ("f(f(1))", 111.0.into()),
            ("[f(1), f(2), f(3)].join()", "11,21,31".into()),
            ("f(1) > 10 ? f(2) : f(3)", 21.0.into()),
            ("[5, 6, 7][f(0) + 1]", 7.0.into()),
            ("1 + f(2) * 2", 43.0.into()),
        ] {
            assert_eq!(
                run(&format!("{f} let r = {expr};"), "r"),
                expected,
                "{expr}"
            );
        }
        let src = format!("{f} let a = [0, 0]; a[f(0) - 1] = f(4); let r = a.join();");
        assert_eq!(run(&src, "r"), "41,0".into());
    }

    #[test]
    fn test_function_in_finished_block_is_not_supported_yet() {
        // Calling a function after the scope it was created in has ended
        // needs closures to capture variables.
        let err = run_err("let g; { function inner() { return 1 } g = inner; } g()");
        assert!(matches!(err, JSError::NotSupported(_)), "{err:?}");
        // While the block is running, it works.
        assert_eq!(
            run("let r; { function inner() { return 1 } r = inner(); }", "r"),
            1.0.into()
        );
    }

    #[test]
    fn test_caller_resumes_in_its_own_scopes_after_a_call() {
        for (src, name, expected) in [
            // Calls from inside block scopes, which the callee's own blocks
            // must not disturb.
            (
                "function f(x) { { let y = x * 2; if (y > 2) { let z = y; return z } } }
                 let r = 0;
                 { let a = 1; { let b = 2; r = f(a + b) + a + b; } r = r + a; }",
                "r",
                JsValue::from(10.0),
            ),
            // A call in a loop condition and body, resuming at the right place.
            (
                "function lt(a, b) { return a < b } function inc(x) { return x + 1 }
                 let n = 0; for (let i = 0; lt(i, 5); i = inc(i)) { let k = inc(n); n = k; }",
                "n",
                5.0.into(),
            ),
            // Falling off the end of a nested call gives undefined to its caller.
            (
                "function none() { } function outer() { let v = none(); return v }
                 let r = 1; r = outer()",
                "r",
                JsValue::Undefined,
            ),
        ] {
            assert_eq!(run(src, name), expected, "{src}");
        }
    }

    #[test]
    fn test_deep_recursion_throws_instead_of_overflowing() {
        let down = "function down(n) { if (n == 0) { return 0 } return down(n - 1) + 1 }";
        let deepest = MAX_CALL_DEPTH - 1;
        assert_eq!(
            run(&format!("{down} let r = down({deepest})"), "r"),
            (deepest as f64).into()
        );
        let err = run_err(&format!("{down} down({})", MAX_CALL_DEPTH * 100));
        assert!(matches!(err, JSError::RangeError(_)), "{err:?}");
        // Deeper than the native stack allowed when calls recursed in Rust.
        let r = run(&format!("{down} let r = down({deepest}) + down(5)"), "r");
        assert_eq!(r, ((deepest + 5) as f64).into());
        // The depth unwinds with the error, so later calls still work.
        let mut engine = JSEngine::new();
        engine.verbose = false;
        assert!(engine.exec(&format!("{down} down(100000)")).is_err());
        assert!(engine.exec(&format!("{down} down(10)")).is_ok());
    }

    #[test]
    fn test_unary_ops() {
        let f = "function f() { }";
        for (src, expected) in [
            ("let x = -5", JsValue::from(-5.0)),
            ("let y = 3; let x = -y", (-3.0).into()),
            ("let a = 2; let x = -a * 3", (-6.0).into()),
            ("let a = 2; let x = -(a + 1)", (-3.0).into()),
            ("let x = 1 / -0", f64::NEG_INFINITY.into()),
            ("let x = +'42'", 42.0.into()),
            ("let s = ' 42 '; let x = +s", 42.0.into()),
            ("let s = 'a'; let x = +s", f64::NAN.into()),
            ("let x = !0", true.into()),
            ("let v = 'a'; let x = !v", false.into()),
            ("let v = 'a'; let x = !!v", true.into()),
            ("let x = ~5", (-6.0).into()),
            ("let v = 0 - 1; let x = ~v", 0.0.into()),
            ("let v = 2 ** 32 + 3; let x = ~v", (-4.0).into()),
            ("let x = typeof 1", "number".into()),
            ("let x = typeof 'a'", "string".into()),
            ("let x = typeof true", "boolean".into()),
            ("let x = typeof null", "object".into()),
            ("let x = typeof [1]", "object".into()),
            ("let x = typeof notDeclared", "undefined".into()),
            ("let x = typeof console.log", "function".into()),
            (&format!("{f} let x = typeof f"), "function".into()),
            ("let v = 1; let x = typeof (v + 'a')", "string".into()),
            ("let n = 0; let x = void (n = 5); x = n", 5.0.into()),
        ] {
            assert_eq!(run(src, "x"), expected, "{src}");
        }
        assert_eq!(run("let n = 1; let x = void n", "x"), JsValue::Undefined);
        let err = run_err("let a = [1]; delete a[0]");
        assert!(matches!(err, JSError::NotSupported(_)), "{err:?}");
    }

    #[test]
    fn test_update_ops() {
        for (src, name, expected) in [
            ("let i = 1; let r = i++", "r", JsValue::from(1.0)),
            ("let i = 1; let r = i++", "i", 2.0.into()),
            ("let i = 1; let r = ++i", "r", 2.0.into()),
            ("let i = 1; let r = ++i", "i", 2.0.into()),
            ("let i = 5; let r = i--", "r", 5.0.into()),
            ("let i = 5; let r = i--", "i", 4.0.into()),
            ("let i = 5; let r = --i", "r", 4.0.into()),
            ("let i = 1; let r = (i)++ + i", "r", 3.0.into()),
            ("let i = 1; let r = i++ + i++ + ++i", "r", 7.0.into()),
            // The old value is converted to a number before it's returned.
            ("let s = '5'; let r = s++", "r", 5.0.into()),
            ("let s = '5'; let r = s++", "s", 6.0.into()),
            // Elements, with the index evaluated once.
            (
                "let a = [1, 2]; let r = a[0]++; let s = a.join()",
                "r",
                1.0.into(),
            ),
            (
                "let a = [1, 2]; let r = a[0]++; let s = a.join()",
                "s",
                "2,2".into(),
            ),
            (
                "let a = [1, 2]; let r = ++a[1]; let s = a.join()",
                "r",
                3.0.into(),
            ),
            (
                "let a = [1, 2]; let r = --a[1] * 10 + a[1]",
                "r",
                11.0.into(),
            ),
            (
                "let a = [1, 2, 3]; let i = 0; a[i++]++; let s = a.join()",
                "s",
                "2,2,3".into(),
            ),
            ("let a = [1, 2, 3]; let i = 0; a[i++]++", "i", 1.0.into()),
            (
                "let a = [[5]]; let r = a[0][0]++ + a[0][0]",
                "r",
                11.0.into(),
            ),
            // In loops.
            (
                "let s = 0; for (let i = 0; i < 5; i++) { s += i }",
                "s",
                10.0.into(),
            ),
            (
                "let n = 0; for (let i = 10; i > 0; --i) { n++ }",
                "n",
                10.0.into(),
            ),
        ] {
            assert_eq!(run(src, name), expected, "{src}");
        }
        let err = run_err("const c = 1; c++");
        assert!(matches!(err, JSError::InvalidReassign(_)), "{err:?}");
        let err = run_err("notDeclared++");
        assert!(matches!(err, JSError::ReferenceError(_)), "{err:?}");
    }

    #[test]
    fn test_sequence_expressions() {
        for (src, name, expected) in [
            ("let x = (1, 2, 3)", "x", JsValue::from(3.0)),
            ("let a = 0; let x = (a = 5, a + 1)", "x", 6.0.into()),
            ("let a = 0; let x = (a = 5, a)", "x", 5.0.into()),
            ("let a = 1; let x = (a++, a++, a)", "x", 3.0.into()),
            (
                "let n = 0; for (let i = 0, j = 10; i < j; i++, j--) { n++ }",
                "n",
                5.0.into(),
            ),
        ] {
            assert_eq!(run(src, name), expected, "{src}");
        }
    }

    #[test]
    fn test_while_and_do_while() {
        for (src, name, expected) in [
            (
                "let i = 0; let s = 0; while (i < 5) { s += i; i++ }",
                "s",
                JsValue::from(10.0),
            ),
            ("let n = 0; while (false) { n++ }", "n", 0.0.into()),
            ("let i = 3; while (i) i--", "i", 0.0.into()),
            (
                "let i = 0; let s = ''; while (i < 3) { let c = i * 2; s += c; i++ }",
                "s",
                "024".into(),
            ),
            // The body runs once before the test.
            ("let n = 0; do { n++ } while (false)", "n", 1.0.into()),
            ("let n = 0; do n += 2; while (n < 7)", "n", 8.0.into()),
        ] {
            assert_eq!(run(src, name), expected, "{src}");
        }
    }

    #[test]
    fn test_for_of() {
        for (src, name, expected) in [
            (
                "let s = 0; for (const x of [1, 2, 3]) { s += x }",
                "s",
                JsValue::from(6.0),
            ),
            (
                "let s = ''; for (let c of 'abc') s = c + s",
                "s",
                "cba".into(),
            ),
            ("let n = 0; for (const x of []) { n++ }", "n", 0.0.into()),
            (
                "let s = ''; for (const [k, v] of [['a', 1], ['b', 2]]) { s += k + v }",
                "s",
                "a1b2".into(),
            ),
            ("let x = 0; for (x of [4, 5]) { } ", "x", 5.0.into()),
            ("for (var v of [7, 8]) { }", "v", 8.0.into()),
            // Elements added while looping are visited.
            (
                "let a = [1]; let s = ''; for (const x of a) { s += x; if (x < 3) { a.push(x + 1) } }",
                "s",
                "123".into(),
            ),
            (
                "let s = ''; for (const x of [1, 2]) { for (const y of ['a', 'b']) { s += x + y } }",
                "s",
                "1a1b2a2b".into(),
            ),
            // The loop's variable is gone afterwards.
            (
                "let x = 'outer'; for (const x of [1]) { } let r = x",
                "r",
                "outer".into(),
            ),
        ] {
            assert_eq!(run(src, name), expected, "{src}");
        }
    }

    #[test]
    fn test_break_and_continue() {
        for (src, name, expected) in [
            (
                "let i = 0; for (; ; i++) { if (i == 3) { break } }",
                "i",
                JsValue::from(3.0),
            ),
            (
                "let s = 0; for (let i = 0; i < 6; i++) { if (i % 2) { continue } s += i }",
                "s",
                6.0.into(),
            ),
            (
                "let i = 0; while (true) { i++; if (i > 4) break }",
                "i",
                5.0.into(),
            ),
            (
                "let i = 0; let s = 0; while (i < 5) { i++; if (i == 2) continue; s += i }",
                "s",
                13.0.into(),
            ),
            // `continue` in a do-while goes to the test.
            (
                "let i = 0; let n = 0; do { i++; if (i < 3) continue; n++ } while (i < 5)",
                "n",
                3.0.into(),
            ),
            (
                "let s = ''; for (const c of 'abcdef') { if (c == 'b') continue; if (c == 'e') break; s += c }",
                "s",
                "acd".into(),
            ),
            // Labels reach outer loops.
            (
                "let n = 0; outer: for (let i = 0; i < 3; i++) { for (let j = 0; j < 3; j++) { if (j == 1) continue outer; if (i == 2) break outer; n++ } }",
                "n",
                2.0.into(),
            ),
            (
                "let s = ''; a: b: for (const x of [1, 2, 3]) { for (const y of [1, 2]) { if (y == 2) continue a; if (x == 3) break b; s += x } }",
                "s",
                "12".into(),
            ),
            (
                "let r = 0; block: { r = 1; if (r) { break block } r = 2 }",
                "r",
                1.0.into(),
            ),
            // Jumping out of block scopes ends them: `x` is the outer one
            // again after the loop.
            (
                "let x = 'outer'; for (let i = 0; i < 3; i++) { let x = 'loop'; { let y = i; if (y == 1) { break } } } let r = x",
                "r",
                "outer".into(),
            ),
            (
                "let x = 'outer'; let n = 0; while (n < 100) { let x = n; n++; { let y = x; if (y >= 0) { continue } } } let r = x + n",
                "r",
                "outer100".into(),
            ),
            (
                "let x = 'outer'; out: { let x = 'block'; for (const v of [1]) { let w = v; break out } } let r = x",
                "r",
                "outer".into(),
            ),
            (
                "function f() { for (const x of [1, 2, 3]) { let y = x; if (y == 2) { return y } } } let r = f()",
                "r",
                2.0.into(),
            ),
        ] {
            assert_eq!(run(src, name), expected, "{src}");
        }
        for src in [
            "break",
            "if (true) { continue }",
            "for (;;) { function f() { break } break }",
            "while (false) { break nowhere }",
            "a: { for (;;) { continue a } }",
            "switch (1) { case 1: continue }",
        ] {
            let err = run_err(src);
            assert!(matches!(err, JSError::BadJavascript(_)), "{src}: {err:?}");
        }
    }

    #[test]
    fn test_switch() {
        let classify = "function classify(v) {
            let r = '';
            switch (v) {
                case 1: r += 'one'; break;
                case 2: r += 'two';
                case 3: r += 'three'; break;
                default: r += 'other';
                case 4: r += 'four';
            }
            return r
        }";
        for (arg, expected) in [
            ("1", "one"),
            ("2", "twothree"),
            ("3", "three"),
            ("4", "four"),
            ("5", "otherfour"),
            // Cases match with `===`.
            ("'1'", "otherfour"),
        ] {
            let src = format!("{classify} let r = classify({arg})");
            assert_eq!(run(&src, "r"), expected.into(), "classify({arg})");
        }
        for (src, name, expected) in [
            (
                "let r = 0; switch (3) { case 1: r = 1 }",
                "r",
                JsValue::from(0.0),
            ),
            (
                "let r = 0; switch (1 + 1) { case 2: { let r2 = 5; r = r2 } }",
                "r",
                5.0.into(),
            ),
            (
                "let r = 0; switch ('a') { case 'a': let x = 7; r = x; break; case 'b': }",
                "r",
                7.0.into(),
            ),
            // Cases are tested in order, stopping at the first match.
            (
                "let n = 0; function t(v) { n++; return v } switch (2) { case t(1): case t(2): case t(3): }",
                "n",
                2.0.into(),
            ),
            // `continue` in a switch continues the loop around it.
            (
                "let s = ''; for (const c of 'abc') { switch (c) { case 'b': continue; default: s += c } }",
                "s",
                "ac".into(),
            ),
            (
                "let s = 0; for (let i = 0; i < 4; i++) { switch (i % 2) { case 0: s += 10; break; case 1: s += 1 } }",
                "s",
                22.0.into(),
            ),
        ] {
            assert_eq!(run(src, name), expected, "{src}");
        }
    }

    #[test]
    fn test_trivial_statements() {
        assert_eq!(
            run("let x = 1;; ; debugger; x; 5; -1; 'a';", "x"),
            1.0.into()
        );
        let err = run_err("notDeclared;");
        assert!(matches!(err, JSError::ReferenceError(_)), "{err:?}");
    }

    #[test]
    fn test_symbols() {
        for (src, expected) in [
            ("let r = typeof Symbol.iterator", JsValue::from("symbol")),
            ("let r = Symbol.iterator === Symbol.iterator", true.into()),
            (
                "let r = Symbol.iterator === Symbol.asyncIterator",
                false.into(),
            ),
            ("let r = Symbol() === Symbol()", false.into()),
            ("let s = Symbol('x'); let r = s === s", true.into()),
            ("let r = typeof Symbol('x')", "symbol".into()),
            (
                "let r = Symbol.iterator.toString()",
                "Symbol(Symbol.iterator)".into(),
            ),
            (
                "let r = Symbol.asyncIterator.toString()",
                "Symbol(Symbol.asyncIterator)".into(),
            ),
            ("let r = Symbol.iterator in [1]", false.into()),
            (
                "let r = Symbol.toString()",
                "function Symbol() { [native code] }".into(),
            ),
        ] {
            assert_eq!(run(src, "r"), expected, "{src}");
        }
        let err = run_err("let s = Symbol(); let r = s + 1");
        assert!(matches!(err, JSError::TypeError(_)), "{err:?}");
        let err = run_err("console()");
        assert!(matches!(err, JSError::TypeError(_)), "{err:?}");
    }

    #[test]
    fn test_call_apply_bind() {
        let add = "function add(a, b) { return a + b }";
        for (src, expected) in [
            ("let r = add.call(null, 1, 2)", JsValue::from(3.0)),
            ("let r = add.call()", f64::NAN.into()),
            ("let r = add.apply(null, [1, 2])", 3.0.into()),
            ("let r = add.apply(null)", f64::NAN.into()),
            ("let inc = add.bind(null, 1); let r = inc(5)", 6.0.into()),
            ("let r = add.bind(null, 1, 2)()", 3.0.into()),
            (
                "let inc = add.bind(null, 1); let r = inc.call(null, 10)",
                11.0.into(),
            ),
            ("let r = add.bind(null, 1).bind(null, 2)()", 3.0.into()),
            // `bind` makes a new function and leaves the original alone.
            (
                "let inc = add.bind(null, 100); let r = add(1, 2)",
                3.0.into(),
            ),
            ("let r = add.bind(null) === add", false.into()),
            ("let r = add.length", 2.0.into()),
            ("let r = add.bind(null, 1).length", 1.0.into()),
            ("let r = typeof add.bind(null)", "function".into()),
        ] {
            let src = format!("{add} {src}");
            assert_eq!(run(&src, "r"), expected, "{src}");
        }
        for (src, expected) in [
            (
                "function f(a, b = 1, c) { } let r = f.length",
                JsValue::from(1.0),
            ),
            ("function f(a, ...rest) { } let r = f.length", 1.0.into()),
            (
                "function f() { return 7 } let fns = [f]; let r = fns[0]()",
                7.0.into(),
            ),
            // A built-in's bound `this` is the object it runs on.
            (
                "let a = [1, 2]; let push = a.push.bind(a); push(3); push(4); let r = a.join()",
                "1,2,3,4".into(),
            ),
            ("let a = [3, 1]; let r = a.join.call(a, '-')", "3-1".into()),
            (
                "let r = [].concat.apply([1], [[2], [3]]).join()",
                "1,2,3".into(),
            ),
            (
                "let log = console.log.bind(console); log('(bound console.log)'); let r = 1",
                1.0.into(),
            ),
            // Calls through `call` run on the frame stack, not in Rust.
            (
                "function down(n) { if (n == 0) { return 0 } return down.call(null, n - 1) + 1 } let r = down(5000)",
                5000.0.into(),
            ),
        ] {
            assert_eq!(run(src, "r"), expected, "{src}");
        }
        for src in [
            "let push = [].push; push(1)",
            "function f() { } f.apply(null, 5)",
        ] {
            let err = run_err(src);
            assert!(matches!(err, JSError::TypeError(_)), "{src}: {err:?}");
        }
    }

    #[test]
    fn test_binary_op_errors_propagate() {
        let mut engine = JSEngine::new();
        assert!(matches!(
            engine.exec("let r = 'a' in 1"),
            Err(JSError::TypeError(_))
        ));
    }
}
