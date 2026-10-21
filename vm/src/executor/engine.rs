use std::{collections::HashMap, path::Display, rc::Rc};

use rjs_core::{
    expr::{AssignOp, BinaryOp, Expr},
    literal::Literal,
    pattern::Pattern,
    run_parser,
    statement::{Statement, VarDecl},
};

use crate::{
    error::JSError,
    executor::{
        instr::{Binary, Instruction, Set, Value},
        scope::{Scope, VariableType},
    },
    primitive::{RcPrimitive, jstack::JSStack},
};

#[derive(Debug, Clone)]
pub enum InstructionType {
    Instruction(Vec<Instruction>),
    Value(Value),
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
    pub registers: HashMap<Register, Option<RcPrimitive>>,
    pub stack: JSStack,
}

#[allow(clippy::new_without_default)]
impl JSEngine {
    pub fn new() -> Self {
        let registers = HashMap::from([
            (Register::AX, None),
            (Register::BX, None),
            (Register::CX, None),
            (Register::DX, None),
        ]);
        Self {
            scope: Rc::new(Scope::default()),
            registers,
            stack: JSStack::default(),
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
        for instr in instrs {
            instr.exec(self, &scope)?;
        }
        Ok(())
    }

    fn emit_code(
        &mut self,
        scope: &Rc<Scope>,
        stmts: &[Statement],
    ) -> Result<Vec<Instruction>, JSError> {
        let mut instrs = vec![];
        for st in stmts {
            instrs.extend(self.emit_statement(scope, st)?);
        }
        Ok(instrs)
    }

    fn emit_statement(
        &mut self,
        scope: &Rc<Scope>,
        st: &Statement,
    ) -> Result<Vec<Instruction>, JSError> {
        match st {
            Statement::VarDecl(v) => Ok(self.emit_vardecl(scope, v)?),
            Statement::Expr(ex) => {
                let ins = self.emit_exprs(scope, ex, None)?;
                if let InstructionType::Instruction(v) = ins {
                    Ok(v)
                } else {
                    Err(JSError::UnknownError)
                }
            }
            _ => todo!(),
        }
    }

    fn emit_vardecl(
        &mut self,
        scope: &Rc<Scope>,
        decl: &VarDecl,
    ) -> Result<Vec<Instruction>, JSError> {
        let mut instrs = vec![];
        for d in &decl.decls {
            match &d.target {
                Pattern::Ident(id) => {
                    if let Some(ex) = &d.init {
                        let ins = self.emit_exprs(scope, ex, None)?;
                        if let InstructionType::Value(v) = ins {
                            instrs.push(Instruction::Set(Set {
                                l_value: super::instr::Value::Var(id.value.clone()),
                                r_value: Some(v),
                            }));
                        } else if let InstructionType::Instruction(ins) = ins {
                            // Assigning from a register only updates an
                            // existing variable, so declare it first.
                            scope.add_variable(&id.value, VariableType::Any, None)?;
                            instrs.extend(ins);
                            instrs.push(Instruction::Set(Set {
                                l_value: super::instr::Value::Var(id.value.clone()),
                                r_value: Some(Value::Register(Register::AX)),
                            }));
                        }
                    }
                }
                _ => todo!(),
            }
        }
        Ok(instrs)
    }

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
            Expr::Paren(_expr) => todo!(),
            Expr::Spread(_expr) => todo!(),
            Expr::Unary { op, value } => todo!(),
            Expr::Update { op, prefix, target } => todo!(),
            Expr::Binary { left, op, right } => {
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
                            ins.push(Instruction::Set(Set {
                                l_value: Value::Var(tmp.clone()),
                                r_value: Some(Value::Register(reg)),
                            }));
                            left = Value::Var(tmp);
                        }
                        ins.extend(r);
                        Value::Register(Register::AX)
                    }
                };
                ins.push(Instruction::Binary(Binary {
                    left,
                    op: *op,
                    right,
                    output,
                }));
                Ok(InstructionType::Instruction(ins))
            }
            Expr::Conditional {
                test,
                then,
                otherwise,
            } => todo!(),
            Expr::Assign { target, op, value } => {
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
                            ins.push(Instruction::Binary(Binary {
                                left: Value::Var(v.clone()),
                                op: bin,
                                right: rv,
                                output: Some(Register::AX),
                            }));
                            Value::Register(Register::AX)
                        }
                    };
                    ins.push(Instruction::Set(Set {
                        l_value: Value::Var(v),
                        r_value: Some(rv),
                    }));
                    Ok(InstructionType::Instruction(ins))
                } else {
                    todo!()
                }
            }
            Expr::Sequence(exprs) => todo!(),
            Expr::Member {
                object,
                property,
                optional,
            } => todo!(),
            Expr::Index {
                object,
                index,
                optional,
            } => todo!(),
            Expr::Call {
                callee,
                args,
                optional,
            } => todo!(),
            Expr::New { callee, args } => todo!(),
            Expr::Yield {
                span,
                delegate,
                value,
            } => todo!(),
        }
    }

    fn get_value_from_expr(&mut self, scope: &Rc<Scope>, ex: &Expr) -> Option<Value> {
        match ex {
            Expr::Literal(l) => Some(Value::Primitive(RcPrimitive::from(l))),
            Expr::Ident(i) => scope.get_variable(&i.value)?.into(),
            _ => {
                let s = format!("{:?} not implemented", ex);
                println!("{s}");
                todo!()
            }
        }
    }

    pub fn set_register(&mut self, register: Register, value: Option<RcPrimitive>) {
        let _ = self.registers.entry(register).and_modify(|e| *e = value);
    }

    pub fn get_register(&self, register: Register) -> Option<RcPrimitive> {
        self.registers.get(&register).and_then(|v| v.clone())
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

#[cfg(test)]
mod tests {

    use super::*;

    const SOURCE: &str = "let a =1; let b =2 ; b = a+b+10; "; //console.log(a); let a = c";

    #[test]
    fn test_engine() {
        let mut engine = JSEngine::new();
        engine.exec(SOURCE).unwrap();
        assert!(engine.scope.get_variable("a") == Some(RcPrimitive::from(1.0)));
        assert!(engine.scope.get_variable("b") == Some(RcPrimitive::from(13.0)));
    }

    /// Run `src` and return the value of variable `name` afterwards.
    fn run(src: &str, name: &str) -> RcPrimitive {
        let mut engine = JSEngine::new();
        engine.exec(src).unwrap();
        engine
            .scope
            .get_variable(name)
            .unwrap_or_else(|| panic!("`{name}` has no value after: {src}"))
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

    #[test]
    fn test_engine1() {
        let mut engine = JSEngine::new();
        engine.set_register(Register::AX, Some(0.1.into()));
        println!("{engine}");
    }
}
