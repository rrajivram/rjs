use std::{collections::HashMap, rc::Rc};

use rjs_core::{
    expr::{AssignOp, BinaryOp, Expr},
    ident::Ident,
    literal::Literal,
    pattern::Pattern,
    run_parser,
    statement::{Statement, VarDecl},
};

use crate::{
    error::JSError,
    executor::{
        instr::{Binary, Call, GetAttr, Instruction, Jump, JumpIf, JumpPos, Set, Value},
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
        let scope = self.scope.clone();
        let instrs = self.emit_code(&scope, &res.program)?;
        let mut pc = 0;
        while pc < instrs.len() {
            pc = match instrs[pc].as_ref().exec(self, &scope)? {
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

    fn emit_code(
        &mut self,
        scope: &Rc<Scope>,
        stmts: &[Statement],
    ) -> Result<Vec<LabelledInstruction>, JSError> {
        self.labels.clear();
        let mut instrs = vec![];
        for st in stmts {
            instrs.extend(self.emit_statement(scope, st)?);
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

    fn emit_statement(
        &mut self,
        scope: &Rc<Scope>,
        st: &Statement,
    ) -> Result<Vec<LabelledInstruction>, JSError> {
        match st {
            Statement::VarDecl(v) => Ok(self.emit_vardecl(scope, v)?),
            Statement::Expr(ex) => {
                let ins = self.emit_exprs(scope, ex, None)?;
                if let InstructionType::Instruction(v) = ins {
                    Ok(v)
                } else {
                    Err(JSError::UnknownError(
                        "Did not exepct Value in InstructionType".into(),
                    ))
                }
            }
            _ => todo!(),
        }
    }

    fn emit_vardecl(
        &mut self,
        scope: &Rc<Scope>,
        decl: &VarDecl,
    ) -> Result<Vec<LabelledInstruction>, JSError> {
        let mut instrs = vec![];
        for d in &decl.decls {
            match &d.target {
                Pattern::Ident(id) => {
                    if let Some(ex) = &d.init {
                        let ins = self.emit_exprs(scope, ex, None)?;
                        if let InstructionType::Value(v) = ins {
                            instrs.push(
                                Instruction::Set(Set {
                                    l_value: super::instr::Value::Var(id.value.clone()),
                                    r_value: v,
                                })
                                .into(),
                            );
                        } else if let InstructionType::Instruction(ins) = ins {
                            // Assigning from a register only updates an
                            // existing variable, so declare it first.
                            scope.add_variable(
                                &id.value,
                                VariableType::Any,
                                Primitive::Undefined.into(),
                            )?;
                            instrs.extend(ins);
                            instrs.push(
                                Instruction::Set(Set {
                                    l_value: super::instr::Value::Var(id.value.clone()),
                                    r_value: Value::Register(Register::AX),
                                })
                                .into(),
                            );
                        }
                    }
                }
                _ => todo!(),
            }
        }
        Ok(instrs)
    }

    #[allow(unused)]
    fn emit_exprs(
        &mut self,
        scope: &Rc<Scope>,
        ex: &Expr,
        output: Option<Register>,
    ) -> Result<InstructionType, JSError> {
        println!("In Exprs: {:?}", ex);
        match ex {
            Expr::Literal(literal) => Ok(InstructionType::Value(literal.into())),
            Expr::Ident(ident) => Ok(InstructionType::Value(Value::Var(ident.value.clone()))),
            Expr::This(_token_span) => todo!(),
            Expr::Super(_token_span) => todo!(),
            Expr::Array(_exprs) => todo!(),
            Expr::Object(_items) => todo!(),
            Expr::Function(_function) => todo!(),
            Expr::Arrow(_arrow) => todo!(),
            Expr::Class(_class) => todo!(),
            Expr::Paren(expr) => Ok(self.emit_exprs(scope, expr, output)?),
            Expr::Spread(_expr) => todo!(),
            Expr::Unary { op, value } => todo!(),
            Expr::Update { op, prefix, target } => todo!(),
            Expr::Binary { left, op, right } => {
                Ok(self.emit_binary(left, op, right, scope, output)?)
            }
            Expr::Conditional {
                test,
                then,
                otherwise,
            } => Ok(self.emit_conditional(test, then, otherwise, scope)?),
            Expr::Assign { target, op, value } => Ok(self.emit_assign(target, op, value, scope)?),
            Expr::Sequence(exprs) => todo!(),
            Expr::Member {
                object,
                property,
                optional,
            } => Ok(self.emit_member(object, property, *optional, scope)?),
            Expr::Index {
                object,
                index,
                optional,
            } => todo!(),
            Expr::Call {
                callee,
                args,
                optional,
            } => Ok(self.emit_call(callee, args, *optional, scope)?),
            Expr::New { callee, args } => todo!(),
            Expr::Yield {
                span,
                delegate,
                value,
            } => todo!(),
        }
    }

    fn emit_conditional(
        &mut self,
        test: &Expr,
        then: &Expr,
        otherwise: &Expr,
        scope: &Rc<Scope>,
    ) -> Result<InstructionType, JSError> {
        let mut ins = vec![];
        let test = self.emit_exprs(scope, test, None)?;
        let then = self.emit_exprs(scope, then, None)?;
        let otherwise = self.emit_exprs(scope, otherwise, None)?;
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
        scope: &Rc<Scope>,
    ) -> Result<InstructionType, JSError> {
        let ident = self.emit_exprs(scope, target, None)?;
        if let InstructionType::Value(Value::Var(v)) = ident {
            let (mut ins, rv) = match self.emit_exprs(scope, value, None)? {
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
        scope: &Rc<Scope>,
        output: Option<Register>,
    ) -> Result<InstructionType, JSError> {
        let left = self.emit_exprs(scope, left, None)?;
        let right = self.emit_exprs(scope, right, None)?;
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
                    let tmp = scope.new_temp_var();
                    ins.push(
                        Instruction::Set(Set {
                            l_value: Value::Var(tmp.clone()),
                            r_value: Value::Register(reg),
                        })
                        .into(),
                    );
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
        scope: &Rc<Scope>,
    ) -> Result<InstructionType, JSError> {
        let ins = self.emit_exprs(scope, object, None)?;
        if let InstructionType::Value(m) = ins {
            let mut ins = vec![];
            let get = GetAttr {
                object: m,
                attrib: prop.value.clone(),
                output: Value::Register(Register::AX),
                obj_output: Value::Register(Register::BX),
            };
            ins.push(Instruction::GetAttr(get).into());
            Ok(InstructionType::Instruction(ins))
        } else {
            todo!()
        }
    }

    fn emit_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        _optional: bool,
        scope: &Rc<Scope>,
    ) -> Result<InstructionType, JSError> {
        let mut names = vec![];
        let mut ins = vec![];
        for arg in args {
            let name = scope.new_temp_var();
            names.push(Value::Var(name.clone()));
            let var = self.emit_exprs(scope, arg, Some(Register::BX))?;
            match var {
                InstructionType::Instruction(i) => {
                    ins.extend(i);
                    ins.push(
                        self.set_instr(Value::Var(name), Value::Register(Register::BX))
                            .into(),
                    );
                }
                InstructionType::Value(v) => {
                    ins.push(self.set_instr(Value::Var(name), v).into());
                }
            }
        }
        let callee = self.emit_exprs(scope, callee, None)?;
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

    pub fn new_label(&mut self) -> String {
        let label = format!("__L{}", self.labels.len());
        self.labels.insert(label.clone(), None);
        label
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

    const SOURCE: &str = "let a =1; let b =2 ; let c = a > b? 'yes':'no'; let d = a < b? 'yes':'no';  console.log(c + ' ' + d);";

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
    fn test_binary_op_errors_propagate() {
        let mut engine = JSEngine::new();
        assert!(matches!(
            engine.exec("let r = 'a' in 1"),
            Err(JSError::TypeError(_))
        ));
    }
}
