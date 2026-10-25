use std::{
    fmt::Display,
    hash::{Hash, Hasher},
    ops::Add,
    rc::Rc,
};

use rjs_core::literal::{Literal, NumberValue};

use crate::{error::JSError, executor::instr::Value, primitive::objects::Object};

pub mod jstack;
pub mod non_numeric;
pub mod numeric;
pub mod objects;
pub mod ops;

#[derive(Debug, Clone)]
pub enum Primitive {
    Number(f64),
    BigInt(i128),
    Str(String),
    Bool(bool),
    Null,
    Undefined,
    Symbol(u64),
    Object(Rc<dyn Object>),
    Function(Func),
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Func(pub String);

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct RcPrimitive(pub Rc<Primitive>);

impl PartialEq for Primitive {
    fn eq(&self, other: &Self) -> bool {
        use Primitive::*;
        match (self, other) {
            (Number(a), Number(b)) => a == b || (a.is_nan() && b.is_nan()),
            (BigInt(a), BigInt(b)) => a == b,
            (Str(a), Str(b)) => a == b,
            (Bool(a), Bool(b)) => a == b,
            (Symbol(a), Symbol(b)) => a == b,
            (Null, Null) | (Undefined, Undefined) => true,
            (Object(a), Object(b)) => Rc::ptr_eq(a, b),
            (Function(s1), Function(s2)) => s1 == s2,
            _ => false,
        }
    }
}

impl Eq for Primitive {}

impl Hash for Primitive {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            // Equal values must hash equally: fold -0 into 0 and all NaNs into one.
            Primitive::Number(n) => {
                let n = if *n == 0.0 {
                    0.0
                } else if n.is_nan() {
                    f64::NAN
                } else {
                    *n
                };
                n.to_bits().hash(state)
            }
            Primitive::BigInt(n) => n.hash(state),
            Primitive::Str(s) => s.hash(state),
            Primitive::Bool(b) => b.hash(state),
            Primitive::Symbol(s) => s.hash(state),
            Primitive::Object(o) => (Rc::as_ptr(o) as *const () as usize).hash(state),
            Primitive::Function(f) => f.hash(state),
            _ => {}
        }
    }
}

impl Display for Primitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Primitive::Number(n) => n.to_string(),
            Primitive::BigInt(n) => n.to_string(),
            Primitive::Str(s) => s.clone(),
            Primitive::Bool(b) => b.to_string(),
            Primitive::Null => "null".into(),
            Primitive::Undefined => "undefined".into(),
            Primitive::Symbol(_) => "symbol".into(),
            Primitive::Object(_object) => "object Object".into(),
            Primitive::Function(f) => format!("Function({f})"),
        };
        f.write_str(&s)
    }
}

impl Default for RcPrimitive {
    fn default() -> Self {
        Self(Rc::new(Primitive::Number(0.)))
    }
}

impl Add for RcPrimitive {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        let res: Self = match (self.0.as_ref(), rhs.0.as_ref()) {
            (Primitive::Number(n), Primitive::Number(n1)) => Primitive::Number(n + n1).into(),
            (Primitive::BigInt(n), Primitive::BigInt(n1)) => Primitive::BigInt(n + n1).into(),
            (Primitive::BigInt(n), Primitive::Number(n1)) => {
                Primitive::BigInt(n + *n1 as i128).into()
            }
            (Primitive::Number(n), Primitive::BigInt(n1)) => {
                Primitive::Number(n + *n1 as f64).into()
            }
            (Primitive::Str(s1), Primitive::Str(s2)) => Primitive::Str(format!("{s1}{s2}")).into(),
            (Primitive::Str(s), Primitive::BigInt(b)) => Primitive::Str(format!("{s}{b}")).into(),
            (Primitive::Str(s), Primitive::Number(b)) => Primitive::Str(format!("{s}{b}")).into(),
            (Primitive::Str(s), Primitive::Bool(b)) => Primitive::Str(format!("{s}{b}")).into(),
            (Primitive::BigInt(s), Primitive::Str(b)) => Primitive::Str(format!("{s}{b}")).into(),
            (Primitive::Number(s), Primitive::Str(b)) => Primitive::Str(format!("{s}{b}")).into(),
            (Primitive::Bool(s), Primitive::Str(b)) => Primitive::Str(format!("{s}{b}")).into(),

            (_, _) => Primitive::Null.into(),
        };
        res
    }
}

impl From<Primitive> for RcPrimitive {
    fn from(value: Primitive) -> Self {
        Self(Rc::new(value))
    }
}

impl Display for RcPrimitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<&Literal> for RcPrimitive {
    fn from(value: &Literal) -> Self {
        let val = match value {
            Literal::Boolean(b) => Primitive::Bool(b.value()),
            Literal::Null(_) => Primitive::Null,
            Literal::Number(n) => match &n.value {
                NumberValue::Float(f) => Primitive::Number(*f),
                NumberValue::Int(i) => Primitive::Number(*i as f64),
            },
            Literal::String(s) => Primitive::Str(s.value.clone()),
        };
        Self(Rc::new(val))
    }
}
impl From<Literal> for RcPrimitive {
    fn from(value: Literal) -> Self {
        Self::from(&value)
    }
}

impl From<f64> for RcPrimitive {
    fn from(value: f64) -> Self {
        Self(Rc::new(Primitive::Number(value)))
    }
}

impl From<bool> for RcPrimitive {
    fn from(value: bool) -> Self {
        Self(Rc::new(Primitive::Bool(value)))
    }
}

impl From<String> for RcPrimitive {
    fn from(value: String) -> Self {
        Self(Rc::new(Primitive::Str(value)))
    }
}

impl From<&str> for RcPrimitive {
    fn from(value: &str) -> Self {
        Self(Rc::new(Primitive::Str(value.to_string())))
    }
}

impl AsRef<Primitive> for RcPrimitive {
    fn as_ref(&self) -> &Primitive {
        &self.0
    }
}

impl AsRef<str> for Func {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Func {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

impl From<String> for Func {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl Display for Func {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)?;
        Ok(())
    }
}

impl Primitive {
    pub fn to_str(&self) -> String {
        format!("{self}")
    }
}

impl Object for Primitive {
    fn get_props(&self) -> Vec<String> {
        todo!()
    }

    fn get_methods(&self) -> Vec<String> {
        todo!()
    }

    fn get_prop(&self, prop: &str) -> Result<RcPrimitive, crate::error::JSError> {
        if let Self::Object(obj) = self {
            Ok(obj.get_prop(prop)?)
        } else {
            todo!()
        }
    }

    fn set_prop(&self, prop: &str, value: RcPrimitive) -> Result<(), JSError> {
        if let Self::Object(obj) = self {
            Ok(obj.set_prop(prop, value)?)
        } else {
            todo!()
        }
    }

    fn has_prop(&self, _prop: &str) -> bool {
        todo!()
    }

    fn has_method(&self, _method: &str) -> bool {
        todo!()
    }

    fn call(
        &self,
        engine: &mut crate::executor::engine::JSEngine,
        scope: &Rc<crate::executor::scope::Scope>,
        method: &Func,
        args: &[Value],
    ) -> Result<(), crate::error::JSError> {
        if let Self::Object(ob) = self {
            Ok(ob.call(engine, scope, method, args)?)
        } else {
            todo!()
        }
    }

    fn value_of(&self) -> Result<Primitive, JSError> {
        let res = self.to_number();
        match res {
            Ok(f) => Ok(Primitive::Number(f)),
            Err(_) => Ok(Primitive::Null),
        }
    }

    fn type_of(&self) -> String {
        match self {
            Primitive::Number(_) => "Number".into(),
            Primitive::BigInt(_) => "BigInteger".into(),
            Primitive::Str(_) => "String".into(),
            Primitive::Bool(_) => "Boolean".into(),
            Primitive::Null => "(Null)".into(),
            Primitive::Undefined => "(Undefined)".into(),
            Primitive::Symbol(_) => "Symbol".into(),
            Primitive::Object(object) => object.type_of(),
            Primitive::Function(_func) => "Function".into(),
        }
    }
}
