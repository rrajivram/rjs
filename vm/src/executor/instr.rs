use std::{ops::Add, sync::Arc};

use rjs_core::{expr::BinaryOp, ident::ObjectName};

use crate::{
    error::JSError,
    executor::{
        engine::{JSEngine, Register},
        scope::Scope,
    },
    primitive::{ArcPrimitive, Primitive},
};

#[derive(Debug, Clone)]
pub enum Instruction {
    Set(Set),
    Binary(Binary),
}

#[derive(Debug, Clone)]
pub struct Binary {
    pub left: Value,
    pub op: BinaryOp,
    pub right: Value,
    pub output: Option<Register>,
}

#[derive(Debug, Clone)]
pub struct Set {
    pub l_value: Value,
    pub r_value: Option<Value>,
}

#[derive(Debug, Clone)]
pub enum Value {
    Register(Register),
    Var(String),
    Primitive(ArcPrimitive),
}

impl Instruction {
    pub fn exec(&self, engine: &mut JSEngine, scope: &Arc<Scope>) -> Result<(), JSError> {
        match self {
            Self::Set(s) => self.set(engine, scope, s)?,
            Self::Binary(b) => self.binary(engine, scope, b)?,
        }
        Ok(())
    }

    fn set(&self, engine: &mut JSEngine, scope: &Arc<Scope>, set: &Set) -> Result<(), JSError> {
        match (&set.l_value, &set.r_value) {
            (Value::Var(r), Some(Value::Primitive(p))) => scope.add_variable(
                r,
                crate::executor::scope::VariableType::Any,
                Some(p.clone()),
            )?,
            (Value::Var(v), Some(Value::Register(r))) => {
                scope.set_variable(v, engine.get_register(*r))?
            }
            (Value::Var(v1), Some(Value::Var(v2))) => {
                scope.set_variable(v1, scope.get_variable(v2))?
            }
            (Value::Register(r1), Some(Value::Register(r2))) => {
                engine.set_register(*r1, engine.get_register(*r2));
            }
            (Value::Register(r), Some(Value::Var(v))) => {
                engine.set_register(*r, scope.get_variable(v));
            }
            (Value::Register(r), Some(Value::Primitive(p))) => {
                engine.set_register(*r, Some(p.clone()))
            }
            (_, _) => todo!(),
        }
        Ok(())
    }

    fn binary(
        &self,
        engine: &mut JSEngine,
        scope: &Arc<Scope>,
        binary: &Binary,
    ) -> Result<(), JSError> {
        let reg = match &binary.op {
            BinaryOp::Add => binary.left.do_add(&binary.right, engine, scope),
            BinaryOp::Sub => todo!(),
            BinaryOp::Mul => todo!(),
            BinaryOp::Div => todo!(),
            BinaryOp::Mod => todo!(),
            BinaryOp::Exp => todo!(),
            BinaryOp::Shl => todo!(),
            BinaryOp::Shr => todo!(),
            BinaryOp::UShr => todo!(),
            BinaryOp::Lt => todo!(),
            BinaryOp::Gt => todo!(),
            BinaryOp::LtEq => todo!(),
            BinaryOp::GtEq => todo!(),
            BinaryOp::In => todo!(),
            BinaryOp::InstanceOf => todo!(),
            BinaryOp::Eq => todo!(),
            BinaryOp::NotEq => todo!(),
            BinaryOp::StrictEq => todo!(),
            BinaryOp::StrictNotEq => todo!(),
            BinaryOp::BitAnd => todo!(),
            BinaryOp::BitOr => todo!(),
            BinaryOp::BitXor => todo!(),
            BinaryOp::And => todo!(),
            BinaryOp::Or => todo!(),
            BinaryOp::Nullish => todo!(),
        }?;
        let register = if let Some(reg) = binary.output {
            reg
        } else {
            Register::AX
        };
        engine.set_register(register, Some(reg));
        Ok(())
    }
}

impl From<ArcPrimitive> for Value {
    fn from(value: ArcPrimitive) -> Self {
        Self::Primitive(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Primitive(ArcPrimitive::from(value))
    }
}

impl Value {
    fn do_add(
        &self,
        rhs: &Self,
        engine: &mut JSEngine,
        scope: &Arc<Scope>,
    ) -> Result<ArcPrimitive, JSError> {
        let (p1, p2) = self.get_vars(&rhs, engine, scope);
        let v = match (p1, p2) {
            (Some(p1), Some(p2)) => p1 + p2,
            (Some(p1), _) => p1,
            (_, Some(p2)) => p2,
            (None, None) => ArcPrimitive::from(Primitive::Undefined),
        };
        Ok(v.into())
    }

    fn get_vars(
        &self,
        rhs: &Self,
        engine: &mut JSEngine,
        scope: &Arc<Scope>,
    ) -> (Option<ArcPrimitive>, Option<ArcPrimitive>) {
        let p1 = match self {
            Value::Primitive(p) => Some(p.clone()),
            Value::Register(r) => engine.get_register(*r),
            Value::Var(v) => scope.get_variable(&v),
        };
        let p2 = match rhs {
            Value::Primitive(p) => Some(p.clone()),
            Value::Register(r) => engine.get_register(*r),
            Value::Var(v) => scope.get_variable(&v),
        };
        (p1, p2)
    }

    pub fn do_from(value: ArcPrimitive) -> Self {
        value.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::{
        engine::{JSEngine, Register},
        instr::Instruction,
    };

    fn engine() -> JSEngine {
        JSEngine::new()
    }

    #[test]
    fn test_exec_set() {
        let mut engine = engine();
        let set = Instruction::Set(Set {
            l_value: Value::Register(Register::AX),
            r_value: Some(ArcPrimitive::from(1.0).into()),
        });
        let scope = engine.scope.clone();
        set.exec(&mut engine, &scope).unwrap();
        assert!(engine.get_register(Register::AX) == Some(ArcPrimitive::from(1.0)));
        let set = Instruction::Set(Set {
            l_value: Value::Var("test".into()),
            r_value: Some(ArcPrimitive::from(2.0).into()),
        });
        set.exec(&mut engine, &scope).unwrap();
        assert!(scope.get_variable("test") == Some(ArcPrimitive::from(2.0)));
    }

    #[test]
    fn test_add() {
        let mut engine = engine();
        let binary = Instruction::Binary(Binary {
            left: 1.0.into(),
            op: BinaryOp::Add,
            right: 2.0.into(),
            output: Some(Register::BX),
        });
        let scope = engine.scope.clone();
        binary.exec(&mut engine, &scope).unwrap();
        assert_eq!(engine.get_register(Register::BX), Some(3.0.into()))
    }
}
