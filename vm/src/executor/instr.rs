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
            op => binary.left.do_op(&binary.right, engine, scope, |a, b| match op {
                BinaryOp::Add => unreachable!("handled by do_add"),
                BinaryOp::Sub => a - b,
                BinaryOp::Mul => a * b,
                BinaryOp::Div => a / b,
                BinaryOp::Mod => a % b,
                BinaryOp::Exp => a.pow(&b),
                BinaryOp::Shl => a << b,
                BinaryOp::Shr => a >> b,
                BinaryOp::UShr => a.unsigned_shr(&b),
                BinaryOp::Lt => a.less_than(&b),
                BinaryOp::Gt => a.greater_than(&b),
                BinaryOp::LtEq => a.less_eq(&b),
                BinaryOp::GtEq => a.greater_eq(&b),
                BinaryOp::In => a.is_in(&b),
                BinaryOp::InstanceOf => a.instance_of(&b),
                BinaryOp::Eq => Ok(a.loose_equals(&b).into()),
                BinaryOp::NotEq => Ok((!a.loose_equals(&b)).into()),
                BinaryOp::StrictEq => Ok(a.strict_equals(&b).into()),
                BinaryOp::StrictNotEq => Ok((!a.strict_equals(&b)).into()),
                BinaryOp::BitAnd => a & b,
                BinaryOp::BitOr => a | b,
                BinaryOp::BitXor => a ^ b,
                BinaryOp::And => Ok(a.logical_and(b)),
                BinaryOp::Or => Ok(a.logical_or(b)),
                BinaryOp::Nullish => Ok(a.nullish_or(b)),
            }),
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

    /// Resolve both operands and apply `op`. A variable or register with no
    /// value reads as `undefined`.
    fn do_op(
        &self,
        rhs: &Self,
        engine: &mut JSEngine,
        scope: &Arc<Scope>,
        op: impl FnOnce(ArcPrimitive, ArcPrimitive) -> Result<ArcPrimitive, JSError>,
    ) -> Result<ArcPrimitive, JSError> {
        let (p1, p2) = self.get_vars(rhs, engine, scope);
        op(
            p1.unwrap_or_else(ArcPrimitive::undefined),
            p2.unwrap_or_else(ArcPrimitive::undefined),
        )
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
    use crate::{
        executor::{
            engine::{JSEngine, Register},
            instr::Instruction,
        },
        primitive::objects::Object,
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

    /// Run `left op right` as a `Binary` instruction and return the result.
    fn eval(
        left: impl Into<ArcPrimitive>,
        op: BinaryOp,
        right: impl Into<ArcPrimitive>,
    ) -> Result<ArcPrimitive, JSError> {
        let mut engine = engine();
        let scope = engine.scope.clone();
        Instruction::Binary(Binary {
            left: Value::Primitive(left.into()),
            op,
            right: Value::Primitive(right.into()),
            output: Some(Register::CX),
        })
        .exec(&mut engine, &scope)?;
        Ok(engine.get_register(Register::CX).expect("result register is set"))
    }

    fn num(left: impl Into<ArcPrimitive>, op: BinaryOp, right: impl Into<ArcPrimitive>) -> f64 {
        match eval(left, op, right).unwrap().as_ref() {
            Primitive::Number(n) => *n,
            other => panic!("expected a number, got {other:?}"),
        }
    }

    fn boolean(left: impl Into<ArcPrimitive>, op: BinaryOp, right: impl Into<ArcPrimitive>) -> bool {
        match eval(left, op, right).unwrap().as_ref() {
            Primitive::Bool(b) => *b,
            other => panic!("expected a boolean, got {other:?}"),
        }
    }

    fn big(n: i128) -> Primitive {
        Primitive::BigInt(n)
    }

    fn bigint(left: impl Into<ArcPrimitive>, op: BinaryOp, right: impl Into<ArcPrimitive>) -> i128 {
        match eval(left, op, right).unwrap().as_ref() {
            Primitive::BigInt(n) => *n,
            other => panic!("expected a BigInt, got {other:?}"),
        }
    }

    fn type_error(left: impl Into<ArcPrimitive>, op: BinaryOp, right: impl Into<ArcPrimitive>) -> String {
        match eval(left, op, right) {
            Err(JSError::TypeError(m)) => m,
            other => panic!("expected a TypeError, got {other:?}"),
        }
    }

    fn range_error(left: impl Into<ArcPrimitive>, op: BinaryOp, right: impl Into<ArcPrimitive>) -> String {
        match eval(left, op, right) {
            Err(JSError::RangeError(m)) => m,
            other => panic!("expected a RangeError, got {other:?}"),
        }
    }

    const UNDEFINED: Primitive = Primitive::Undefined;
    const NULL: Primitive = Primitive::Null;

    #[test]
    fn test_sub() {
        assert_eq!(num(5.0, BinaryOp::Sub, 7.0), -2.0);
        assert_eq!(num("6", BinaryOp::Sub, 2.0), 4.0);
        assert_eq!(num(true, BinaryOp::Sub, 1.0), 0.0);
        assert_eq!(num(NULL, BinaryOp::Sub, 1.0), -1.0);
        assert!(num(UNDEFINED, BinaryOp::Sub, 1.0).is_nan());
        assert!(num("abc", BinaryOp::Sub, 1.0).is_nan());
        assert_eq!(bigint(big(7), BinaryOp::Sub, big(10)), -3);
    }

    #[test]
    fn test_mul() {
        assert_eq!(num(6.0, BinaryOp::Mul, 7.0), 42.0);
        assert_eq!(num(" 0x10 ", BinaryOp::Mul, 2.0), 32.0);
        assert_eq!(num("", BinaryOp::Mul, 5.0), 0.0);
        assert_eq!(num("Infinity", BinaryOp::Mul, 1.0), f64::INFINITY);
        assert!(num("inf", BinaryOp::Mul, 1.0).is_nan());
        assert_eq!(bigint(big(6), BinaryOp::Mul, big(-7)), -42);
        assert_eq!(range_error(big(i128::MAX), BinaryOp::Mul, big(2)), "Maximum BigInt size exceeded");
    }

    #[test]
    fn test_div() {
        assert_eq!(num(7.0, BinaryOp::Div, 2.0), 3.5);
        assert_eq!(num(1.0, BinaryOp::Div, 0.0), f64::INFINITY);
        assert_eq!(num(-1.0, BinaryOp::Div, 0.0), f64::NEG_INFINITY);
        assert!(num(0.0, BinaryOp::Div, 0.0).is_nan());
        assert_eq!(bigint(big(7), BinaryOp::Div, big(2)), 3);
        assert_eq!(bigint(big(-7), BinaryOp::Div, big(2)), -3);
        assert_eq!(range_error(big(1), BinaryOp::Div, big(0)), "Division by zero");
    }

    #[test]
    fn test_mod() {
        assert_eq!(num(7.0, BinaryOp::Mod, 3.0), 1.0);
        assert_eq!(num(-7.0, BinaryOp::Mod, 3.0), -1.0);
        assert_eq!(num(7.5, BinaryOp::Mod, 2.0), 1.5);
        assert!(num(1.0, BinaryOp::Mod, 0.0).is_nan());
        assert_eq!(bigint(big(-7), BinaryOp::Mod, big(2)), -1);
        assert_eq!(bigint(big(i128::MIN), BinaryOp::Mod, big(-1)), 0);
        assert_eq!(range_error(big(1), BinaryOp::Mod, big(0)), "Division by zero");
    }

    #[test]
    fn test_exp() {
        assert_eq!(num(2.0, BinaryOp::Exp, 10.0), 1024.0);
        assert_eq!(num(4.0, BinaryOp::Exp, 0.5), 2.0);
        assert_eq!(num(2.0, BinaryOp::Exp, -1.0), 0.5);
        assert_eq!(num(f64::NAN, BinaryOp::Exp, 0.0), 1.0);
        assert!(num(1.0, BinaryOp::Exp, f64::NAN).is_nan());
        assert!(num(-1.0, BinaryOp::Exp, f64::INFINITY).is_nan());
        assert_eq!(bigint(big(2), BinaryOp::Exp, big(100)), 1 << 100);
        assert_eq!(bigint(big(-1), BinaryOp::Exp, big(1_000_000_000_001)), -1);
        assert_eq!(range_error(big(2), BinaryOp::Exp, big(-1)), "Exponent must be non-negative");
        assert_eq!(range_error(big(2), BinaryOp::Exp, big(200)), "Maximum BigInt size exceeded");
    }

    #[test]
    fn test_shifts() {
        assert_eq!(num(1.0, BinaryOp::Shl, 4.0), 16.0);
        assert_eq!(num(1.0, BinaryOp::Shl, 31.0), -2147483648.0);
        assert_eq!(num(1.0, BinaryOp::Shl, 33.0), 2.0, "shift count is taken mod 32");
        assert_eq!(num(-8.0, BinaryOp::Shr, 1.0), -4.0);
        assert_eq!(num(-1.0, BinaryOp::UShr, 0.0), 4294967295.0);
        assert_eq!(num(-16.0, BinaryOp::UShr, 28.0), 15.0);
        assert_eq!(bigint(big(1), BinaryOp::Shl, big(100)), 1 << 100);
        assert_eq!(bigint(big(-9), BinaryOp::Shr, big(1)), -5);
        assert_eq!(bigint(big(8), BinaryOp::Shl, big(-2)), 2, "a negative count shifts the other way");
        assert_eq!(range_error(big(1), BinaryOp::Shl, big(127)), "Maximum BigInt size exceeded");
        assert_eq!(
            type_error(big(1), BinaryOp::UShr, big(1)),
            "BigInts have no unsigned right shift, use >> instead"
        );
    }

    #[test]
    fn test_bitwise() {
        assert_eq!(num(5.0, BinaryOp::BitAnd, 3.0), 1.0);
        assert_eq!(num(5.0, BinaryOp::BitOr, 3.0), 7.0);
        assert_eq!(num(5.0, BinaryOp::BitXor, 3.0), 6.0);
        assert_eq!(num(1.9, BinaryOp::BitOr, 0.0), 1.0);
        assert_eq!(num(-1.9, BinaryOp::BitOr, 0.0), -1.0);
        assert_eq!(num(4294967296.0, BinaryOp::BitOr, 0.0), 0.0, "wraps to 32 bits");
        assert_eq!(num(f64::NAN, BinaryOp::BitOr, 0.0), 0.0);
        assert_eq!(num("12", BinaryOp::BitAnd, 10.0), 8.0);
        assert_eq!(bigint(big(12), BinaryOp::BitAnd, big(10)), 8);
        assert_eq!(bigint(big(-1), BinaryOp::BitXor, big(5)), -6);
    }

    #[test]
    fn test_bigint_mixed_with_number_is_a_type_error() {
        for op in [
            BinaryOp::Sub,
            BinaryOp::Mul,
            BinaryOp::Div,
            BinaryOp::Mod,
            BinaryOp::Exp,
            BinaryOp::Shl,
            BinaryOp::Shr,
            BinaryOp::BitAnd,
            BinaryOp::BitOr,
            BinaryOp::BitXor,
        ] {
            assert_eq!(
                type_error(big(1), op, 1.0),
                "Cannot mix BigInt and other types, use explicit conversions",
                "{op}"
            );
        }
    }

    #[test]
    fn test_relational() {
        assert!(boolean(1.0, BinaryOp::Lt, 2.0));
        assert!(!boolean(2.0, BinaryOp::Lt, 2.0));
        assert!(boolean(2.0, BinaryOp::LtEq, 2.0));
        assert!(boolean(3.0, BinaryOp::Gt, 2.0));
        assert!(boolean(2.0, BinaryOp::GtEq, 2.0));
        assert!(boolean("10", BinaryOp::Lt, "9"), "two strings compare as text");
        assert!(!boolean("10", BinaryOp::Lt, 9.0), "a string and a number compare as numbers");
        assert!(boolean("a", BinaryOp::Lt, "b"));
        assert!(boolean("Z", BinaryOp::Lt, "a"), "by code unit, so uppercase first");
        assert!(boolean(NULL, BinaryOp::GtEq, 0.0));
        assert!(!boolean(UNDEFINED, BinaryOp::Lt, 1.0));
        for op in [BinaryOp::Lt, BinaryOp::Gt, BinaryOp::LtEq, BinaryOp::GtEq] {
            assert!(!boolean(f64::NAN, op, 1.0), "NaN {op} 1");
            assert!(!boolean(1.0, op, f64::NAN), "1 {op} NaN");
        }
        assert!(boolean(big(2), BinaryOp::Gt, 1.0));
        assert!(boolean(big(1), BinaryOp::Lt, 1.5));
        assert!(boolean(big(1), BinaryOp::LtEq, 1.0));
        assert!(boolean(big(10), BinaryOp::Gt, "9"));
        assert!(!boolean(big(1), BinaryOp::Lt, f64::NAN));
    }

    #[test]
    fn test_equality() {
        assert!(boolean(1.0, BinaryOp::Eq, "1"));
        assert!(boolean(0.0, BinaryOp::Eq, ""));
        assert!(boolean(true, BinaryOp::Eq, 1.0));
        assert!(boolean("1", BinaryOp::Eq, true));
        assert!(boolean(NULL, BinaryOp::Eq, UNDEFINED));
        assert!(!boolean(NULL, BinaryOp::Eq, 0.0));
        assert!(!boolean(UNDEFINED, BinaryOp::Eq, false));
        assert!(!boolean(f64::NAN, BinaryOp::Eq, f64::NAN));
        assert!(boolean(big(1), BinaryOp::Eq, 1.0));
        assert!(boolean("1", BinaryOp::Eq, big(1)));
        assert!(!boolean(1.0, BinaryOp::NotEq, "1"));
        assert!(boolean(NULL, BinaryOp::NotEq, 0.0));

        assert!(boolean(1.0, BinaryOp::StrictEq, 1.0));
        assert!(boolean(0.0, BinaryOp::StrictEq, -0.0));
        assert!(boolean("a", BinaryOp::StrictEq, "a"));
        assert!(!boolean(1.0, BinaryOp::StrictEq, "1"));
        assert!(!boolean(f64::NAN, BinaryOp::StrictEq, f64::NAN));
        assert!(!boolean(NULL, BinaryOp::StrictEq, UNDEFINED));
        assert!(!boolean(big(1), BinaryOp::StrictEq, 1.0));
        assert!(boolean(1.0, BinaryOp::StrictNotEq, "1"));
        assert!(!boolean(UNDEFINED, BinaryOp::StrictNotEq, UNDEFINED));
    }

    #[test]
    fn test_objects_compare_by_identity() {
        let mut engine = engine();
        let scope = engine.scope.clone();
        let object = ArcPrimitive::from(Primitive::Object(Object::new(Default::default())));
        let lookalike = ArcPrimitive::from(Primitive::Object(Object::new(Default::default())));
        let same = Instruction::Binary(Binary {
            left: object.clone().into(),
            op: BinaryOp::StrictEq,
            right: object.clone().into(),
            output: Some(Register::AX),
        });
        same.exec(&mut engine, &scope).unwrap();
        assert_eq!(engine.get_register(Register::AX), Some(true.into()));
        let different = Instruction::Binary(Binary {
            left: object.into(),
            op: BinaryOp::Eq,
            right: lookalike.into(),
            output: Some(Register::AX),
        });
        different.exec(&mut engine, &scope).unwrap();
        assert_eq!(engine.get_register(Register::AX), Some(false.into()));
        assert!(boolean(Primitive::Object(Object::new(Default::default())), BinaryOp::Eq, "[object Object]"));
    }

    #[test]
    fn test_logical() {
        assert_eq!(eval(0.0, BinaryOp::And, "x").unwrap(), 0.0.into());
        assert_eq!(eval(1.0, BinaryOp::And, "x").unwrap(), "x".into());
        assert_eq!(eval("", BinaryOp::Or, "default").unwrap(), "default".into());
        assert_eq!(eval("a", BinaryOp::Or, "default").unwrap(), "a".into());
        assert!(num(f64::NAN, BinaryOp::Or, 5.0) == 5.0, "NaN is falsy");
        assert_eq!(eval(NULL, BinaryOp::Nullish, 5.0).unwrap(), 5.0.into());
        assert_eq!(eval(UNDEFINED, BinaryOp::Nullish, 5.0).unwrap(), 5.0.into());
        assert_eq!(eval(0.0, BinaryOp::Nullish, 5.0).unwrap(), 0.0.into(), "?? keeps falsy values");
        assert_eq!(eval("", BinaryOp::Nullish, 5.0).unwrap(), "".into());
    }

    #[test]
    fn test_in() {
        let props = [("a".to_string(), ArcPrimitive::from(1.0)), ("0".to_string(), 2.0.into())];
        let object = || Primitive::Object(Object::new(props.clone().into_iter().collect()));
        assert!(boolean("a", BinaryOp::In, object()));
        assert!(!boolean("b", BinaryOp::In, object()));
        assert!(boolean(0.0, BinaryOp::In, object()), "the key is converted to a string");
        assert_eq!(
            type_error("a", BinaryOp::In, 1.0),
            "Cannot use 'in' operator to search for 'a' in 1"
        );
        assert_eq!(
            type_error("length", BinaryOp::In, "abc"),
            "Cannot use 'in' operator to search for 'length' in abc"
        );
    }

    #[test]
    fn test_instance_of() {
        assert!(!boolean(1.0, BinaryOp::InstanceOf, Primitive::Function));
        assert_eq!(
            type_error(1.0, BinaryOp::InstanceOf, 1.0),
            "Right-hand side of 'instanceof' is not an object"
        );
        assert_eq!(
            type_error(1.0, BinaryOp::InstanceOf, Primitive::Object(Object::new(Default::default()))),
            "Right-hand side of 'instanceof' is not callable"
        );
    }

    #[test]
    fn test_operands_from_variables_and_registers() {
        let mut engine = engine();
        let scope = engine.scope.clone();
        scope
            .add_variable("x", crate::executor::scope::VariableType::Let, Some(10.0.into()))
            .unwrap();
        engine.set_register(Register::BX, Some(4.0.into()));
        Instruction::Binary(Binary {
            left: Value::Var("x".into()),
            op: BinaryOp::Sub,
            right: Value::Register(Register::BX),
            output: None,
        })
        .exec(&mut engine, &scope)
        .unwrap();
        assert_eq!(engine.get_register(Register::AX), Some(6.0.into()), "output defaults to AX");

        Instruction::Binary(Binary {
            left: Value::Var("missing".into()),
            op: BinaryOp::Mul,
            right: 2.0.into(),
            output: Some(Register::CX),
        })
        .exec(&mut engine, &scope)
        .unwrap();
        let missing = engine.get_register(Register::CX).unwrap();
        assert!(matches!(missing.as_ref(), Primitive::Number(n) if n.is_nan()), "unset reads as undefined");
    }
}
