use std::rc::Rc;

use rjs_core::expr::BinaryOp;

use crate::{
    error::JSError,
    executor::{
        engine::{JSEngine, Register},
        scope::{Scope, VariableType},
    },
    primitive::{Primitive, RcPrimitive, jstack::StackValue, objects::Object},
};

#[derive(Debug, Clone)]
pub enum Instruction {
    Noop,
    Hoist(Hoist),
    Init(Init),
    Declare(Declare),
    Set(Set),
    Binary(Binary),
    GetAttr(GetAttr), //  Call(Call)
    SetAttr(SetAttr),
    Call(Call),
    JumpIf(JumpIf),
    Jump(Jump),
    BeginScope,
    EndScope,
    Push(Push),
    Pop(Pop),
}

#[derive(Debug, Clone)]
pub struct Binary {
    pub left: Value,
    pub op: BinaryOp,
    pub right: Value,
    pub output: Option<Register>,
}

#[derive(Debug, Clone)]
pub struct Push {
    pub value: Value,
}

#[derive(Debug, Clone)]
pub struct Pop {
    pub value: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct Call {
    pub object: Option<Value>,
    pub function: Value,
    pub args: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct GetAttr {
    pub object: Value,
    pub attrib: String,
    pub output: Value,
    pub obj_output: Value,
}

#[derive(Debug, Clone)]
pub struct SetAttr {
    pub object: Value,
    pub attrib: String,
    pub input: Value,
}

/// Create a scope's declared name at the start of the scope, as `undefined`,
/// so it's visible before its declaration line. A `var` that already exists
/// in the scope keeps its value (redeclaring a `var` is a no-op).
#[derive(Debug, Clone)]
pub struct Hoist {
    pub name: String,
    pub kind: VariableType,
}

/// Run a declaration's initializer at its line: set the (hoisted) variable,
/// even if it's `const`.
#[derive(Debug, Clone)]
pub struct Init {
    pub name: String,
    pub value: Value,
}

/// Create a variable in the current scope, initialized to `value`
/// (`undefined` when declared without an initializer).
#[derive(Debug, Clone)]
pub struct Declare {
    pub name: String,
    pub kind: VariableType,
    pub value: Value,
}

/// Assign to an existing variable or a register. Variables are looked up
/// through enclosing scopes; assigning to an undeclared one is an error.
#[derive(Debug, Clone)]
pub struct Set {
    pub l_value: Value,
    pub r_value: Value,
}

#[derive(Debug, Clone)]
pub enum Value {
    Register(Register),
    Var(String),
    Primitive(RcPrimitive),
}

#[derive(Debug, Clone)]
pub struct JumpIf {
    pub if_true: bool,
    pub value: Value,
    pub pos: JumpPos,
}

#[derive(Debug, Clone)]
pub struct Jump {
    pub pos: JumpPos,
}

#[derive(Debug, Clone)]
pub enum JumpPos {
    Global(usize),
    Relative(isize),
    Label(String),
}

impl Instruction {
    pub fn exec(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
    ) -> Result<Option<JumpPos>, JSError> {
        let res = match self {
            Self::Noop => None,
            Self::Hoist(h) => self.hoist(scope, h)?,
            Self::Init(i) => self.init(engine, scope, i)?,
            Self::Declare(d) => self.declare(engine, scope, d)?,
            Self::Set(s) => self.set(engine, scope, s)?,
            Self::Binary(b) => self.binary(engine, scope, b)?,
            Self::GetAttr(g) => self.get_attr(engine, scope, g)?,
            Self::SetAttr(s) => self.set_attr(engine, scope, s)?,
            Self::Call(c) => self.call(engine, scope, c)?,
            Self::JumpIf(j) => self.jump_if(engine, scope, j)?,
            Self::Jump(j) => self.jump(engine, scope, j)?,
            Self::Push(p) => self.push(engine, scope, p)?,
            Self::Pop(p) => self.pop(engine, scope, p)?,

            _ => None,
        };
        Ok(res)
    }

    fn push(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        push: &Push,
    ) -> Result<Option<JumpPos>, JSError> {
        let value = engine.resolve_value(&push.value, scope)?;
        engine.stack.push(value);
        Ok(None)
    }

    fn pop(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        pop: &Pop,
    ) -> Result<Option<JumpPos>, JSError> {
        let v = engine.stack.pop();
        if let Some(v) = v
            && let Some(out) = &pop.value
        {
            let set = Set {
                l_value: out.clone(),
                r_value: Value::Primitive(v),
            };
            self.set(engine, scope, &set)?;
        }
        Ok(None)
    }

    fn jump(
        &self,
        _engine: &mut JSEngine,
        _scope: &Rc<Scope>,
        jump: &Jump,
    ) -> Result<Option<JumpPos>, JSError> {
        Ok(Some(jump.pos.clone()))
    }

    fn jump_if(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        jump: &JumpIf,
    ) -> Result<Option<JumpPos>, JSError> {
        let value = engine
            .resolve_value(&jump.value, scope)?
            .as_ref()
            .to_boolean();
        if value == jump.if_true {
            Ok(Some(jump.pos.clone()))
        } else {
            Ok(None)
        }
    }

    #[allow(clippy::collapsible_if)]
    fn call(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        call: &Call,
    ) -> Result<Option<JumpPos>, JSError> {
        if let Primitive::Function(func) = engine.resolve_value(&call.function, scope)?.as_ref() {
            if let Some(object) = &call.object {
                let object = engine.resolve_value(object, scope)?;
                object.as_ref().call(engine, scope, func, &call.args)?;
                return Ok(None);
            }
        }
        Err(JSError::UnknownError("Call error".into()))
    }

    fn get_attr(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        get: &GetAttr,
    ) -> Result<Option<JumpPos>, JSError> {
        let source = engine.resolve_value(&get.object, scope)?;
        let set = Set {
            l_value: get.output.clone(),
            r_value: Value::Primitive(source.as_ref().get_prop(&get.attrib)?),
        };
        self.set(engine, scope, &set)?;
        // Use the object resolved above: re-resolving `get.object` would read
        // AX after it was overwritten with the property when the object was
        // itself in AX.
        let set = Set {
            l_value: get.obj_output.clone(),
            r_value: Value::Primitive(source),
        };
        self.set(engine, scope, &set)?;
        Ok(None)
    }

    fn set_attr(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        set: &SetAttr,
    ) -> Result<Option<JumpPos>, JSError> {
        let dest = engine.resolve_value(&set.object, scope)?;
        match &set.input {
            Value::Primitive(p) => dest.as_ref().set_prop(&set.attrib, p.clone()),
            Value::Register(r) => dest.as_ref().set_prop(&set.attrib, engine.get_register(*r)),
            Value::Var(v) => dest.as_ref().set_prop(&set.attrib, scope.get_variable(v)?),
        }?;
        Ok(None)
    }

    fn hoist(&self, scope: &Rc<Scope>, hoist: &Hoist) -> Result<Option<JumpPos>, JSError> {
        if hoist.kind == VariableType::Any && scope.has_own_variable(&hoist.name) {
            return Ok(None);
        }
        scope.add_variable(&hoist.name, hoist.kind.clone(), Primitive::Undefined.into())?;
        Ok(None)
    }

    fn init(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        init: &Init,
    ) -> Result<Option<JumpPos>, JSError> {
        let value = engine.resolve_value(&init.value, scope)?;
        scope.init_variable(&init.name, value)?;
        Ok(None)
    }

    fn declare(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        declare: &Declare,
    ) -> Result<Option<JumpPos>, JSError> {
        let value = engine.resolve_value(&declare.value, scope)?;
        scope.add_variable(&declare.name, declare.kind.clone(), value)?;
        Ok(None)
    }

    fn set(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        set: &Set,
    ) -> Result<Option<JumpPos>, JSError> {
        match (&set.l_value, &set.r_value) {
            (Value::Var(v), Value::Primitive(p)) => scope.set_variable(v, p.clone())?,
            (Value::Var(v), Value::Register(r)) => {
                scope.set_variable(v, engine.get_register(*r))?
            }
            (Value::Var(v1), Value::Var(v2)) => scope.set_variable(v1, scope.get_variable(v2)?)?,
            (Value::Register(r1), Value::Register(r2)) => {
                engine.set_register(*r1, engine.get_register(*r2));
            }
            (Value::Register(r), Value::Var(v)) => {
                engine.set_register(*r, scope.get_variable(v)?);
            }
            (Value::Register(r), Value::Primitive(p)) => engine.set_register(*r, p.clone()),
            (_, _) => todo!(),
        }
        Ok(None)
    }

    fn binary(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        binary: &Binary,
    ) -> Result<Option<JumpPos>, JSError> {
        let reg = match &binary.op {
            BinaryOp::Add => binary.left.do_add(&binary.right, engine, scope),
            op => binary
                .left
                .do_op(&binary.right, engine, scope, |a, b| match op {
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
        engine.set_register(register, reg);
        Ok(None)
    }
}

impl Value {
    fn do_add(
        &self,
        rhs: &Self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
    ) -> Result<RcPrimitive, JSError> {
        let (p1, p2) = self.get_vars(rhs, engine, scope)?;
        Ok(p1 + p2)
    }

    /// Resolve both operands and apply `op`.
    fn do_op(
        &self,
        rhs: &Self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        op: impl FnOnce(RcPrimitive, RcPrimitive) -> Result<RcPrimitive, JSError>,
    ) -> Result<RcPrimitive, JSError> {
        let (p1, p2) = self.get_vars(rhs, engine, scope)?;
        op(p1, p2)
    }

    fn get_vars(
        &self,
        rhs: &Self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
    ) -> Result<(RcPrimitive, RcPrimitive), JSError> {
        Ok((
            engine.resolve_value(self, scope)?,
            engine.resolve_value(rhs, scope)?,
        ))
    }

    pub fn do_from(value: RcPrimitive) -> Self {
        value.into()
    }
}

impl From<Register> for Value {
    fn from(value: Register) -> Self {
        Self::Register(value)
    }
}

impl From<usize> for Value {
    fn from(value: usize) -> Self {
        Self::Primitive(Primitive::Number(value as f64).into())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{
        executor::{
            engine::{JSEngine, Register},
            instr::Instruction,
        },
        primitive::objects::ObjectPrimitive,
    };

    fn engine() -> JSEngine {
        JSEngine::new()
    }

    #[test]
    fn test_exec_set() {
        let mut engine = engine();
        let set = Instruction::Set(Set {
            l_value: Value::Register(Register::AX),
            r_value: RcPrimitive::from(1.0).into(),
        });
        let scope = engine.scope.clone();
        set.exec(&mut engine, &scope).unwrap();
        assert!(engine.get_register(Register::AX) == RcPrimitive::from(1.0));
        let set = Instruction::Set(Set {
            l_value: Value::Var("test".into()),
            r_value: RcPrimitive::from(2.0).into(),
        });
        assert!(
            matches!(
                set.exec(&mut engine, &scope),
                Err(JSError::ObjectNotFound(_))
            ),
            "Set never creates a variable"
        );
        Instruction::Declare(Declare {
            name: "test".into(),
            kind: VariableType::Let,
            value: Register::AX.into(),
        })
        .exec(&mut engine, &scope)
        .unwrap();
        assert!(scope.get_variable("test").unwrap() == RcPrimitive::from(1.0));
        set.exec(&mut engine, &scope).unwrap();
        assert!(scope.get_variable("test").unwrap() == RcPrimitive::from(2.0));
    }

    #[test]
    fn test_declare_const() {
        let mut engine = engine();
        let scope = engine.scope.clone();
        Instruction::Declare(Declare {
            name: "c".into(),
            kind: VariableType::Const,
            value: 1.0.into(),
        })
        .exec(&mut engine, &scope)
        .unwrap();
        assert!(scope.get_variable("c").unwrap() == RcPrimitive::from(1.0));
        let reassign = Instruction::Set(Set {
            l_value: Value::Var("c".into()),
            r_value: 2.0.into(),
        });
        assert!(matches!(
            reassign.exec(&mut engine, &scope),
            Err(JSError::InvalidReassign(_))
        ));
    }

    #[test]
    fn test_stack_holds_values_not_references() {
        let mut engine = engine();
        let scope = engine.scope.clone();
        scope
            .add_variable("x", crate::executor::scope::VariableType::Let, 1.0.into())
            .unwrap();
        engine.set_register(Register::AX, 10.0.into());
        for value in [Value::Register(Register::AX), Value::Var("x".into())] {
            Instruction::Push(Push { value }).exec(&mut engine, &scope).unwrap();
        }
        // Change both sources after pushing.
        engine.set_register(Register::AX, 99.0.into());
        scope.set_variable("x", 99.0.into()).unwrap();
        for (expected, register) in [(1.0, Register::BX), (10.0, Register::CX)] {
            Instruction::Pop(Pop {
                value: Some(Value::Register(register)),
            })
            .exec(&mut engine, &scope)
            .unwrap();
            assert_eq!(engine.get_register(register), expected.into());
        }
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
        assert_eq!(engine.get_register(Register::BX), 3.0.into())
    }

    #[test]
    fn test_get_attr() {
        let mut engine = engine();
        let scope = engine.scope.clone();
        let objprim =
            ObjectPrimitive::new(BTreeMap::from([("prop1".into(), RcPrimitive::from(0.1))]));
        let obj = RcPrimitive::from(Primitive::Object(Rc::new(objprim)));
        scope
            .add_variable("first", crate::executor::scope::VariableType::Any, obj)
            .unwrap();
        let get = Instruction::GetAttr(GetAttr {
            attrib: "prop1".into(),
            object: Value::Var("first".into()),
            output: Value::Register(Register::AX),
            obj_output: Value::Register(Register::BX),
        });
        get.exec(&mut engine, &scope).unwrap();
        assert!(engine.get_register(Register::AX) == RcPrimitive::from(0.1));
    }

    #[test]
    fn test_get_attr1() {
        let mut engine = engine();
        let scope = engine.scope.clone();
        let get = Instruction::GetAttr(GetAttr {
            attrib: "log".into(),
            object: Value::Var("console".into()),
            output: Value::Register(Register::AX),
            obj_output: Value::Register(Register::BX),
        });
        get.exec(&mut engine, &scope).unwrap();
        println!("{engine}")
    }

    #[test]
    fn test_set_attr() {
        let mut engine = engine();
        let scope = engine.scope.clone();
        let objprim = ObjectPrimitive::new(BTreeMap::new());
        let obj = RcPrimitive::from(Primitive::Object(Rc::new(objprim)));
        scope
            .add_variable("first", crate::executor::scope::VariableType::Any, obj)
            .unwrap();
        let get = Instruction::GetAttr(GetAttr {
            attrib: "prop1".into(),
            object: Value::Var("first".into()),
            output: Value::Register(Register::AX),
            obj_output: Value::Register(Register::BX),
        });
        let res = get.exec(&mut engine, &scope);
        assert!(res.is_err());
        let set = Instruction::SetAttr(SetAttr {
            object: Value::Var("first".into()),
            attrib: "prop1".into(),
            input: Value::Primitive(RcPrimitive::from(10.0)),
        });
        set.exec(&mut engine, &scope).unwrap();
        get.exec(&mut engine, &scope).unwrap();
        assert!(engine.get_register(Register::AX) == RcPrimitive::from(10.0));
    }

    /// Run `left op right` as a `Binary` instruction and return the result.
    fn eval(
        left: impl Into<RcPrimitive>,
        op: BinaryOp,
        right: impl Into<RcPrimitive>,
    ) -> Result<RcPrimitive, JSError> {
        let mut engine = engine();
        let scope = engine.scope.clone();
        Instruction::Binary(Binary {
            left: Value::Primitive(left.into()),
            op,
            right: Value::Primitive(right.into()),
            output: Some(Register::CX),
        })
        .exec(&mut engine, &scope)?;
        Ok(engine.get_register(Register::CX))
    }

    fn num(left: impl Into<RcPrimitive>, op: BinaryOp, right: impl Into<RcPrimitive>) -> f64 {
        match eval(left, op, right).unwrap().as_ref() {
            Primitive::Number(n) => *n,
            other => panic!("expected a number, got {other:?}"),
        }
    }

    fn boolean(left: impl Into<RcPrimitive>, op: BinaryOp, right: impl Into<RcPrimitive>) -> bool {
        match eval(left, op, right).unwrap().as_ref() {
            Primitive::Bool(b) => *b,
            other => panic!("expected a boolean, got {other:?}"),
        }
    }

    fn big(n: i128) -> Primitive {
        Primitive::BigInt(n)
    }

    fn bigint(left: impl Into<RcPrimitive>, op: BinaryOp, right: impl Into<RcPrimitive>) -> i128 {
        match eval(left, op, right).unwrap().as_ref() {
            Primitive::BigInt(n) => *n,
            other => panic!("expected a BigInt, got {other:?}"),
        }
    }

    fn type_error(
        left: impl Into<RcPrimitive>,
        op: BinaryOp,
        right: impl Into<RcPrimitive>,
    ) -> String {
        match eval(left, op, right) {
            Err(JSError::TypeError(m)) => m,
            other => panic!("expected a TypeError, got {other:?}"),
        }
    }

    fn range_error(
        left: impl Into<RcPrimitive>,
        op: BinaryOp,
        right: impl Into<RcPrimitive>,
    ) -> String {
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
        assert_eq!(
            range_error(big(i128::MAX), BinaryOp::Mul, big(2)),
            "Maximum BigInt size exceeded"
        );
    }

    #[test]
    fn test_div() {
        assert_eq!(num(7.0, BinaryOp::Div, 2.0), 3.5);
        assert_eq!(num(1.0, BinaryOp::Div, 0.0), f64::INFINITY);
        assert_eq!(num(-1.0, BinaryOp::Div, 0.0), f64::NEG_INFINITY);
        assert!(num(0.0, BinaryOp::Div, 0.0).is_nan());
        assert_eq!(bigint(big(7), BinaryOp::Div, big(2)), 3);
        assert_eq!(bigint(big(-7), BinaryOp::Div, big(2)), -3);
        assert_eq!(
            range_error(big(1), BinaryOp::Div, big(0)),
            "Division by zero"
        );
    }

    #[test]
    fn test_mod() {
        assert_eq!(num(7.0, BinaryOp::Mod, 3.0), 1.0);
        assert_eq!(num(-7.0, BinaryOp::Mod, 3.0), -1.0);
        assert_eq!(num(7.5, BinaryOp::Mod, 2.0), 1.5);
        assert!(num(1.0, BinaryOp::Mod, 0.0).is_nan());
        assert_eq!(bigint(big(-7), BinaryOp::Mod, big(2)), -1);
        assert_eq!(bigint(big(i128::MIN), BinaryOp::Mod, big(-1)), 0);
        assert_eq!(
            range_error(big(1), BinaryOp::Mod, big(0)),
            "Division by zero"
        );
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
        assert_eq!(
            range_error(big(2), BinaryOp::Exp, big(-1)),
            "Exponent must be non-negative"
        );
        assert_eq!(
            range_error(big(2), BinaryOp::Exp, big(200)),
            "Maximum BigInt size exceeded"
        );
    }

    #[test]
    fn test_shifts() {
        assert_eq!(num(1.0, BinaryOp::Shl, 4.0), 16.0);
        assert_eq!(num(1.0, BinaryOp::Shl, 31.0), -2147483648.0);
        assert_eq!(
            num(1.0, BinaryOp::Shl, 33.0),
            2.0,
            "shift count is taken mod 32"
        );
        assert_eq!(num(-8.0, BinaryOp::Shr, 1.0), -4.0);
        assert_eq!(num(-1.0, BinaryOp::UShr, 0.0), 4294967295.0);
        assert_eq!(num(-16.0, BinaryOp::UShr, 28.0), 15.0);
        assert_eq!(bigint(big(1), BinaryOp::Shl, big(100)), 1 << 100);
        assert_eq!(bigint(big(-9), BinaryOp::Shr, big(1)), -5);
        assert_eq!(
            bigint(big(8), BinaryOp::Shl, big(-2)),
            2,
            "a negative count shifts the other way"
        );
        assert_eq!(
            range_error(big(1), BinaryOp::Shl, big(127)),
            "Maximum BigInt size exceeded"
        );
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
        assert_eq!(
            num(4294967296.0, BinaryOp::BitOr, 0.0),
            0.0,
            "wraps to 32 bits"
        );
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
        assert!(
            boolean("10", BinaryOp::Lt, "9"),
            "two strings compare as text"
        );
        assert!(
            !boolean("10", BinaryOp::Lt, 9.0),
            "a string and a number compare as numbers"
        );
        assert!(boolean("a", BinaryOp::Lt, "b"));
        assert!(
            boolean("Z", BinaryOp::Lt, "a"),
            "by code unit, so uppercase first"
        );
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
        let object = RcPrimitive::from(Primitive::Object(Rc::new(ObjectPrimitive::new(
            Default::default(),
        ))));
        let lookalike = RcPrimitive::from(Primitive::Object(Rc::new(ObjectPrimitive::new(
            Default::default(),
        ))));
        let same = Instruction::Binary(Binary {
            left: object.clone().into(),
            op: BinaryOp::StrictEq,
            right: object.clone().into(),
            output: Some(Register::AX),
        });
        same.exec(&mut engine, &scope).unwrap();
        assert_eq!(engine.get_register(Register::AX), true.into());
        let different = Instruction::Binary(Binary {
            left: object.into(),
            op: BinaryOp::Eq,
            right: lookalike.into(),
            output: Some(Register::AX),
        });
        different.exec(&mut engine, &scope).unwrap();
        assert_eq!(engine.get_register(Register::AX), false.into());
        assert!(boolean(
            Primitive::Object(Rc::new(ObjectPrimitive::new(Default::default()))),
            BinaryOp::Eq,
            "[object Object]"
        ));
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
        assert_eq!(
            eval(0.0, BinaryOp::Nullish, 5.0).unwrap(),
            0.0.into(),
            "?? keeps falsy values"
        );
        assert_eq!(eval("", BinaryOp::Nullish, 5.0).unwrap(), "".into());
    }

    #[test]
    fn test_in() {
        let props = [
            ("a".to_string(), RcPrimitive::from(1.0)),
            ("0".to_string(), 2.0.into()),
        ];
        let object = || {
            Primitive::Object(Rc::new(ObjectPrimitive::new(
                props.clone().into_iter().collect(),
            )))
        };
        assert!(boolean("a", BinaryOp::In, object()));
        assert!(!boolean("b", BinaryOp::In, object()));
        assert!(
            boolean(0.0, BinaryOp::In, object()),
            "the key is converted to a string"
        );
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
        assert!(!boolean(
            1.0,
            BinaryOp::InstanceOf,
            Primitive::Function("test".into())
        ));
        assert_eq!(
            type_error(1.0, BinaryOp::InstanceOf, 1.0),
            "Right-hand side of 'instanceof' is not an object"
        );
        assert_eq!(
            type_error(
                1.0,
                BinaryOp::InstanceOf,
                Primitive::Object(Rc::new(ObjectPrimitive::new(Default::default())))
            ),
            "Right-hand side of 'instanceof' is not callable"
        );
    }

    #[test]
    fn test_operands_from_variables_and_registers() {
        let mut engine = engine();
        let scope = engine.scope.clone();
        scope
            .add_variable("x", crate::executor::scope::VariableType::Let, 10.0.into())
            .unwrap();
        engine.set_register(Register::BX, 4.0.into());
        Instruction::Binary(Binary {
            left: Value::Var("x".into()),
            op: BinaryOp::Sub,
            right: Value::Register(Register::BX),
            output: None,
        })
        .exec(&mut engine, &scope)
        .unwrap();
        assert_eq!(
            engine.get_register(Register::AX),
            6.0.into(),
            "output defaults to AX"
        );

        let missing = Instruction::Binary(Binary {
            left: Value::Var("missing".into()),
            op: BinaryOp::Mul,
            right: 2.0.into(),
            output: Some(Register::CX),
        })
        .exec(&mut engine, &scope);
        assert!(
            matches!(&missing, Err(JSError::ReferenceError(name)) if name == "missing"),
            "an undeclared variable is a ReferenceError, got {missing:?}"
        );

        scope
            .add_variable(
                "unset",
                crate::executor::scope::VariableType::Let,
                RcPrimitive::undefined(),
            )
            .unwrap();
        Instruction::Binary(Binary {
            left: Value::Var("unset".into()),
            op: BinaryOp::Mul,
            right: 2.0.into(),
            output: Some(Register::CX),
        })
        .exec(&mut engine, &scope)
        .unwrap();
        assert!(
            matches!(engine.get_register(Register::CX).as_ref(), Primitive::Number(n) if n.is_nan()),
            "undefined * 2 is NaN"
        );
    }
}
