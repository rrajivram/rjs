use std::{collections::HashMap, path::Display, sync::Arc};

use rjs_core::{
    expr::{AssignOp, Expr},
    literal::Literal,
    pattern::Pattern,
    run_parser,
    statement::{Statement, VarDecl},
};

use crate::{
    error::JSError,
    executor::{
        instr::{Binary, Instruction, Set, Value},
        scope::Scope,
    },
    primitive::ArcPrimitive,
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
    pub scope: Arc<Scope>,
    pub registers: HashMap<Register, Option<ArcPrimitive>>,
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
            scope: Arc::new(Scope::default()),
            registers,
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
        scope: &Arc<Scope>,
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
        scope: &Arc<Scope>,
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
        scope: &Arc<Scope>,
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
        scope: &Arc<Scope>,
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
                let right = self.emit_exprs(scope, right, Some(Register::BX))?;
                let left = self.emit_exprs(scope, left, Some(Register::AX))?;
                match (left, right) {
                    (InstructionType::Value(v1), InstructionType::Value(v2)) => {
                        let ins = Instruction::Binary(Binary {
                            left: v1,
                            right: v2,
                            op: *op,
                            output,
                        });
                        Ok(InstructionType::Instruction(vec![ins]))
                    }
                    (InstructionType::Value(v), InstructionType::Instruction(rins)) => {
                        let mut ins = rins;
                        ins.push(Instruction::Binary(Binary {
                            left: v,
                            op: *op,
                            right: Value::Register(Register::BX),
                            output: Some(Register::BX),
                        }));
                        Ok(InstructionType::Instruction(ins))
                    }
                    (InstructionType::Instruction(lins), InstructionType::Value(v)) => {
                        let mut ins = lins;
                        ins.push(Instruction::Binary(Binary {
                            left: Value::Register(Register::AX),
                            op: *op,
                            right: v,
                            output: Some(Register::AX),
                        }));
                        Ok(InstructionType::Instruction(ins))
                    }
                    (InstructionType::Instruction(l), InstructionType::Instruction(r)) => {
                        let mut ins = l;
                        ins.extend(r);
                        ins.push(Instruction::Binary(Binary {
                            left: Value::Register(Register::AX),
                            op: *op,
                            right: Value::Register(Register::BX),
                            output: None,
                        })); // default to AX
                        Ok(InstructionType::Instruction(ins))
                    }
                }
                //if let InstructionType::
            }
            Expr::Conditional {
                test,
                then,
                otherwise,
            } => todo!(),
            Expr::Assign { target, op, value } => {
                let ident = self.emit_exprs(scope, target, None)?;
                assert!(matches!(op, AssignOp::Assign));
                if let InstructionType::Value(Value::Var(v)) = ident {
                    let ins = self.emit_exprs(scope, value, None)?;
                    match ins {
                        InstructionType::Value(rv) => {
                            Ok(InstructionType::Instruction(vec![Instruction::Set(Set {
                                l_value: Value::Var(v),
                                r_value: Some(rv),
                            })]))
                        }
                        InstructionType::Instruction(mut ins) => {
                            ins.push(Instruction::Set(Set {
                                l_value: Value::Var(v),
                                r_value: Some(Value::Register(Register::AX)),
                            }));
                            Ok(InstructionType::Instruction(ins))
                        }
                    }
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

    fn get_value_from_expr(&mut self, scope: &Arc<Scope>, ex: &Expr) -> Option<Value> {
        match ex {
            Expr::Literal(l) => Some(Value::Primitive(ArcPrimitive::from(l))),
            Expr::Ident(i) => scope.get_variable(&i.value)?.into(),
            _ => {
                let s = format!("{:?} not implemented", ex);
                println!("{s}");
                todo!()
            }
        }
    }

    pub fn set_register(&mut self, register: Register, value: Option<ArcPrimitive>) {
        let _ = self.registers.entry(register).and_modify(|e| *e = value);
    }

    pub fn get_register(&self, register: Register) -> Option<ArcPrimitive> {
        self.registers.get(&register).and_then(|v| v.clone())
    }
}

impl std::fmt::Display for JSEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "registers: {:?}", self.registers)?;
        write!(f, "{}", self.scope)
    }
}

impl From<ArcPrimitive> for Option<Value> {
    fn from(value: ArcPrimitive) -> Self {
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
        assert!(engine.scope.get_variable("a") == Some(ArcPrimitive::from(1.0)));
        assert!(engine.scope.get_variable("b") == Some(ArcPrimitive::from(13.0)));
    }

    #[test]
    fn test_engine1() {
        let mut engine = JSEngine::new();
        engine.set_register(Register::AX, Some(0.1.into()));
        println!("{engine}");
    }
}
