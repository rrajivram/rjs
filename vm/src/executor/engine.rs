use std::{
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
};

use rjs_core::{
    expr::{AssignOp, BinaryOp, Expr},
    ident::Ident,
    literal::Literal,
    pattern::Pattern,
    run_parser,
    statement::{DeclKind, ForInit, ForLeft, Statement, VarDecl},
};

use crate::{
    error::JSError,
    executor::{
        instr::{
            Binary, Call, Declare, GetAttr, Hoist, Init, Instruction, Jump, JumpIf, JumpPos, Push,
            Set, Value,
        },
        scope::{Scope, VariableType},
    },
    primitive::{Primitive, RcPrimitive, jstack::JSStack},
};

#[derive(Debug, Clone)]
pub enum InstructionType {
    Instruction(Vec<LabelledInstruction>),
    Value(Value),
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

#[derive(Clone)]
pub struct JSEngine {
    pub scope: Rc<Scope>,
    pub registers: HashMap<Register, RcPrimitive>,
    pub stack: JSStack,
    pub labels: HashMap<String, Option<usize>>,
    /// Counter for compiler temporaries (`__t0`, `__t1`, ...).
    temps: usize,
}

#[allow(clippy::new_without_default)]
impl JSEngine {
    pub fn new() -> Self {
        let registers = HashMap::from([
            (Register::AX, Primitive::Undefined.into()),
            (Register::BX, Primitive::Undefined.into()),
            (Register::CX, Primitive::Undefined.into()),
            (Register::DX, Primitive::Undefined.into()),
        ]);
        Self {
            scope: Scope::create_default(),
            registers,
            stack: JSStack::default(),
            labels: HashMap::new(),
            temps: 0,
        }
    }

    pub fn exec(&mut self, src: &str) -> Result<(), JSError> {
        let res = run_parser(src);
        if !res.errors.is_empty() {
            return Err(JSError::BadJavascript(res.errors));
        }
        for s in &res.program {
            println!("{}", s);
        }
        for s in res.diagnostics {
            println!("Warning: {}", s);
        }
        let mut top_scope = self.scope.clone();
        let mut scopes = VecDeque::new();
        let instrs = self.emit_code(&res.program)?;

        let mut pc = 0;
        while pc < instrs.len() {
            let instr = instrs[pc].as_ref();
            if matches!(instr, Instruction::BeginScope) {
                scopes.push_back(top_scope.clone());
                top_scope = Scope::create(top_scope.clone());
                pc += 1;
                continue;
            }
            if matches!(instr, Instruction::EndScope) {
                top_scope = scopes
                    .pop_back()
                    .ok_or(JSError::UnknownError("Could not find scope to pop.".into()))?;
                pc += 1;
                continue;
            }
            pc = match instrs[pc].as_ref().exec(self, &top_scope)? {
                None => pc + 1,
                Some(JumpPos::Label(l)) => self
                    .labels
                    .get(&l)
                    .copied()
                    .flatten()
                    .ok_or(JSError::UnknownError(format!(" Uknown label : {l}")))?,
                Some(JumpPos::Global(g)) => g,
                Some(JumpPos::Relative(r)) => (pc as isize + r) as usize,
            };
        }
        Ok(())
    }

    fn emit_code(&mut self, stmts: &[Statement]) -> Result<Vec<LabelledInstruction>, JSError> {
        self.labels.clear();
        self.temps = 0;
        let mut instrs = hoist_declarations(stmts, true);
        for st in stmts {
            instrs.extend(self.emit_statement(st)?);
        }
        self.build_labels_map(&instrs)?;
        if self.labels.values().any(|v| v.is_none()) {
            return Err(JSError::UnknownError(
                "Some labels don't have values".into(),
            ));
        }
        Ok(instrs)
    }

    fn build_labels_map(&mut self, ins: &[LabelledInstruction]) -> Result<(), JSError> {
        for (index, i) in ins.iter().enumerate() {
            if let Some(label) = &i.label {
                if let Some(instr) = self.labels.get_mut(label) {
                    *instr = Some(index);
                } else {
                    return Err(JSError::UnknownError(format!(
                        "Could not find label {label}"
                    )));
                }
            }
        }
        Ok(())
    }

    fn emit_statement(&mut self, st: &Statement) -> Result<Vec<LabelledInstruction>, JSError> {
        match st {
            Statement::VarDecl(v) => Ok(self.emit_vardecl(v)?),
            Statement::Expr(ex) => {
                let ins = self.emit_exprs(ex, None)?;
                if let InstructionType::Instruction(v) = ins {
                    Ok(v)
                } else {
                    Err(JSError::UnknownError(
                        "Did not exepct Value in InstructionType".into(),
                    ))
                }
            }
            Statement::Block(b) => Ok(self.emit_block(b)?),
            Statement::If {
                test,
                then,
                otherwise,
            } => Ok(self.emit_if(test, then, otherwise)?),
            _ => todo!(),
        }
    }

    /// The name is already hoisted to the start of its scope, so a declaration
    /// only runs its initializer, here at its own line.
    fn emit_vardecl(&mut self, decl: &VarDecl) -> Result<Vec<LabelledInstruction>, JSError> {
        let mut instrs = vec![];
        let is_var = matches!(decl.kind, DeclKind::Var(_));
        for d in &decl.decls {
            match &d.target {
                Pattern::Ident(id) => {
                    let value = match &d.init {
                        // `var x;` leaves an existing value alone.
                        None if is_var => continue,
                        None => Primitive::Undefined.into(),
                        Some(ex) => match self.emit_exprs(ex, None)? {
                            InstructionType::Value(v) => v,
                            InstructionType::Instruction(ins) => {
                                instrs.extend(ins);
                                Value::Register(Register::AX)
                            }
                        },
                    };
                    instrs.push(
                        Instruction::Init(Init {
                            name: id.value.clone(),
                            value,
                        })
                        .into(),
                    );
                }
                _ => todo!(),
            }
        }
        Ok(instrs)
    }

    #[allow(unused)]
    fn emit_exprs(
        &mut self,
        ex: &Expr,
        output: Option<Register>,
    ) -> Result<InstructionType, JSError> {
        println!("In Exprs: {:?}", ex);
        match ex {
            Expr::Literal(literal) => Ok(InstructionType::Value(literal.into())),
            Expr::Ident(ident) => Ok(InstructionType::Value(Value::Var(ident.value.clone()))),
            Expr::This(_token_span) => todo!(),
            Expr::Super(_token_span) => todo!(),
            Expr::Array(exprs) => Ok(self.emit_array(exprs)?),
            Expr::Object(_items) => todo!(),
            Expr::Function(_function) => todo!(),
            Expr::Arrow(_arrow) => todo!(),
            Expr::Class(_class) => todo!(),
            Expr::Paren(expr) => Ok(self.emit_exprs(expr, output)?),
            Expr::Spread(_expr) => todo!(),
            Expr::Unary { op, value } => todo!(),
            Expr::Update { op, prefix, target } => todo!(),
            Expr::Binary { left, op, right } => Ok(self.emit_binary(left, op, right, output)?),
            Expr::Conditional {
                test,
                then,
                otherwise,
            } => Ok(self.emit_conditional(test, then, otherwise)?),
            Expr::Assign { target, op, value } => Ok(self.emit_assign(target, op, value)?),
            Expr::Sequence(exprs) => todo!(),
            Expr::Member {
                object,
                property,
                optional,
            } => Ok(self.emit_member(object, property, *optional)?),
            Expr::Index {
                object,
                index,
                optional,
            } => todo!(),
            Expr::Call {
                callee,
                args,
                optional,
            } => Ok(self.emit_call_expr(callee, args, *optional)?),
            Expr::New { callee, args } => todo!(),
            Expr::Yield {
                span,
                delegate,
                value,
            } => todo!(),
        }
    }

    fn emit_array(&mut self, array: &[Option<Expr>]) -> Result<InstructionType, JSError> {
        let mut instr = vec![];
        for a in array {
            if let Some(val) = a {
                let ins = self.emit_exprs(val, None)?;
                match ins {
                    InstructionType::Instruction(i) => {
                        instr.extend(i);
                        instr.push(
                            Instruction::Push(super::instr::Push {
                                value: Value::Register(Register::AX),
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
                        value: Value::Primitive(Primitive::Null.into()),
                    })
                    .into(),
                );
            }
        }
        instr.push(
            Instruction::GetAttr(GetAttr {
                object: Value::Var("Array".into()),
                attrib: "new".into(),
                output: Value::Register(Register::AX),
                obj_output: Value::Register(Register::BX),
            })
            .into(),
        );
        instr.push(
            Instruction::Call(Call {
                object: Some(Value::Register(Register::BX)),
                function: Value::Register(Register::AX),
                args: vec![array.len().into()],
            })
            .into(),
        );
        Ok(InstructionType::Instruction(instr))
    }

    fn emit_block(&mut self, block: &[Statement]) -> Result<Vec<LabelledInstruction>, JSError> {
        let mut ins = vec![];
        ins.push(Instruction::BeginScope.into());
        ins.extend(hoist_declarations(block, false));
        for stmt in block {
            ins.extend(self.emit_statement(stmt)?);
        }
        ins.push(Instruction::EndScope.into());
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
                        value: Value::Register(Register::AX),
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

    fn emit_conditional(
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
                ins.push(self.set_instr(Value::Register(Register::AX), v).into());
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
                ins.push(self.set_instr(Value::Register(Register::AX), v).into());
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

    fn emit_assign(
        &mut self,
        target: &Expr,
        op: &AssignOp,
        value: &Expr,
    ) -> Result<InstructionType, JSError> {
        let ident = self.emit_exprs(target, None)?;
        if let InstructionType::Value(Value::Var(v)) = ident {
            let (mut ins, rv) = match self.emit_exprs(value, None)? {
                InstructionType::Value(rv) => (vec![], rv),
                InstructionType::Instruction(ins) => (ins, Value::Register(Register::AX)),
            };
            // `x op= y` is `x = x op y`.
            let rv = match compound_op(*op) {
                None => rv,
                Some(bin) => {
                    ins.push(
                        Instruction::Binary(Binary {
                            left: Value::Var(v.clone()),
                            op: bin,
                            right: rv,
                            output: Some(Register::AX),
                        })
                        .into(),
                    );
                    Value::Register(Register::AX)
                }
            };
            // An assignment is an expression whose value is the assigned
            // value, and an instruction list's result lives in AX.
            if !matches!(rv, Value::Register(Register::AX)) {
                ins.push(
                    self.set_instr(Value::Register(Register::AX), rv.clone())
                        .into(),
                );
            }
            ins.push(
                Instruction::Set(Set {
                    l_value: Value::Var(v),
                    r_value: rv,
                })
                .into(),
            );
            Ok(InstructionType::Instruction(ins))
        } else {
            todo!()
        }
    }

    fn emit_binary(
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
                Value::Register(Register::AX)
            }
        };
        let right = match right {
            InstructionType::Value(v) => v,
            InstructionType::Instruction(r) => {
                // The right side's instructions reuse AX, so move the
                // left result out of the way first.
                if let Value::Register(reg) = left {
                    let tmp = self.new_temp();
                    ins.push(Self::declare_instr(
                        tmp.clone(),
                        VariableType::Any,
                        Value::Register(reg),
                    ));
                    left = Value::Var(tmp);
                }
                ins.extend(r);
                Value::Register(Register::AX)
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

    fn emit_member(
        &mut self,
        object: &Expr,
        prop: &Ident,
        _optional: bool,
    ) -> Result<InstructionType, JSError> {
        // The object may itself need computing (`[1, 2].length`,
        // `a.slice(1).join()`), leaving it in AX.
        let (mut ins, object) = match self.emit_exprs(object, None)? {
            InstructionType::Value(v) => (vec![], v),
            InstructionType::Instruction(i) => (i, Value::Register(Register::AX)),
        };
        ins.push(
            Instruction::GetAttr(GetAttr {
                object,
                attrib: prop.value.clone(),
                output: Value::Register(Register::AX),
                obj_output: Value::Register(Register::BX),
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
            names.push(Value::Var(name.clone()));
            // An instruction list leaves its result in AX. The value is copied
            // into the temp right away, before the next argument reuses AX.
            let value = match self.emit_exprs(arg, None)? {
                InstructionType::Instruction(i) => {
                    ins.extend(i);
                    Value::Register(Register::AX)
                }
                InstructionType::Value(v) => v,
            };
            ins.push(Self::declare_instr(name, VariableType::Any, value));
        }
        let callee = self.emit_exprs(callee, None)?;
        match callee {
            InstructionType::Instruction(i) => {
                ins.extend(i);
                ins.push(
                    Instruction::Call(Call {
                        object: Some(Value::Register(Register::BX)),
                        args: names,
                        function: Value::Register(Register::AX),
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

    pub fn set_instr(&self, l_value: Value, r_value: Value) -> Instruction {
        Instruction::Set(Set { l_value, r_value })
    }

    pub fn set_register(&mut self, register: Register, value: RcPrimitive) {
        let _ = self.registers.entry(register).and_modify(|e| *e = value);
    }

    pub fn get_register(&self, register: Register) -> RcPrimitive {
        self.registers.get(&register).unwrap().clone()
    }

    pub fn resolve_value(&self, value: &Value, scope: &Rc<Scope>) -> Result<RcPrimitive, JSError> {
        let value = match value {
            Value::Primitive(p) => p.clone(),
            Value::Var(v) => scope.get_variable(v)?,
            Value::Register(r) => self.get_register(*r),
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

    fn declare_instr(name: String, kind: VariableType, value: Value) -> LabelledInstruction {
        Instruction::Declare(Declare { name, kind, value }).into()
    }

    pub fn new_label(&mut self) -> String {
        let label = format!("__L{}", self.labels.len());
        self.labels.insert(label.clone(), None);
        label
    }
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
                names.extend(binding_name(&d.target).map(|n| (n, kind.clone())));
            }
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
            out.extend(d.decls.iter().filter_map(|d| binding_name(&d.target)));
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
                out.extend(binding_name(target));
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
fn binding_name(target: &Pattern) -> Option<String> {
    match target {
        Pattern::Ident(id) => Some(id.value.clone()),
        _ => None,
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

impl From<RcPrimitive> for Option<Value> {
    fn from(value: RcPrimitive) -> Self {
        Some(Value::Primitive(value))
    }
}

impl From<&Literal> for Value {
    fn from(value: &Literal) -> Self {
        Self::Primitive(value.into())
    }
}

impl From<RcPrimitive> for Value {
    fn from(value: RcPrimitive) -> Self {
        Value::Primitive(value)
    }
}

impl From<Primitive> for Value {
    fn from(value: Primitive) -> Self {
        Value::Primitive(RcPrimitive::from(value))
    }
}
impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Primitive(RcPrimitive::from(value))
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

    const SOURCE: &str = "let a = [1,2,3]; console.log(a.toString())";

    #[ignore]
    #[test]
    fn test_engine() {
        let mut engine = JSEngine::new();
        engine.exec(SOURCE).unwrap();
    }

    /// Run `src` and return the value of variable `name` afterwards.
    fn run(src: &str, name: &str) -> RcPrimitive {
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
            ("let x = 10; x = 3", RcPrimitive::from(3.0)),
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
            ("let r = 10 - 2 * 3", RcPrimitive::from(4.0)),
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
            ("let x = 1; { x = 3; }", RcPrimitive::from(3.0)),
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
        assert_eq!(run("let x", "x"), Primitive::Undefined.into());
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
                RcPrimitive::from(1.0),
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
        assert_eq!(
            run("let y = x; var x = 3;", "y"),
            Primitive::Undefined.into()
        );
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
    fn test_binary_op_errors_propagate() {
        let mut engine = JSEngine::new();
        assert!(matches!(
            engine.exec("let r = 'a' in 1"),
            Err(JSError::TypeError(_))
        ));
    }
}
