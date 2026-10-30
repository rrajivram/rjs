use std::{
    collections::HashMap,
    fmt::Display,
    hash::{Hash, Hasher},
    ops::Add,
    rc::Rc,
};

use rjs_core::literal::{Literal, NumberValue};

use crate::{
    error::JSError,
    executor::instr::Operand,
    primitive::{
        func::Func,
        objects::Object,
        proto::{Prototype, RcPrototype},
    },
};

pub mod func;
pub mod jstack;
pub mod non_numeric;
pub mod numeric;
pub mod objects;
pub mod ops;
pub mod proto;

/// Any JavaScript value. Cheap to clone: strings, objects and functions are
/// reference-counted, and everything else is stored inline.
#[derive(Debug, Clone)]
pub enum JsValue {
    Number(f64),
    /// Boxed so the rare BigInt doesn't make every value 16-byte aligned.
    BigInt(Rc<i128>),
    Str(Rc<str>),
    Bool(bool),
    Null,
    Undefined,
    Symbol(u64),
    Object(Rc<dyn Object>),
    Function(Func),
}

// Values are copied constantly (registers, variables, arguments, array
// elements), so keep them small: large or rare payloads go behind an `Rc`.
const _: () = assert!(std::mem::size_of::<JsValue>() == 24);

impl PartialEq for JsValue {
    fn eq(&self, other: &Self) -> bool {
        use JsValue::*;
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

impl Eq for JsValue {}

impl Hash for JsValue {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            // Equal values must hash equally: fold -0 into 0 and all NaNs into one.
            JsValue::Number(n) => {
                let n = if *n == 0.0 {
                    0.0
                } else if n.is_nan() {
                    f64::NAN
                } else {
                    *n
                };
                n.to_bits().hash(state)
            }
            JsValue::BigInt(n) => n.hash(state),
            JsValue::Str(s) => s.hash(state),
            JsValue::Bool(b) => b.hash(state),
            JsValue::Symbol(s) => s.hash(state),
            JsValue::Object(o) => (Rc::as_ptr(o) as *const () as usize).hash(state),
            JsValue::Function(f) => f.hash(state),
            _ => {}
        }
    }
}

impl Display for JsValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            JsValue::Number(n) => n.to_string(),
            JsValue::BigInt(n) => n.to_string(),
            JsValue::Str(s) => s.to_string(),
            JsValue::Bool(b) => b.to_string(),
            JsValue::Null => "null".into(),
            JsValue::Undefined => "undefined".into(),
            JsValue::Symbol(_) => "symbol".into(),
            JsValue::Object(_object) => "object Object".into(),
            JsValue::Function(f) => format!("Function({f})"),
        };
        f.write_str(&s)
    }
}

impl Add for JsValue {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        let concat = |a: &dyn Display, b: &dyn Display| JsValue::Str(format!("{a}{b}").into());
        match (&self, &rhs) {
            (JsValue::Number(n), JsValue::Number(n1)) => JsValue::Number(n + n1),
            (JsValue::BigInt(n), JsValue::BigInt(n1)) => JsValue::bigint(**n + **n1),
            (JsValue::BigInt(n), JsValue::Number(n1)) => JsValue::bigint(**n + *n1 as i128),
            (JsValue::Number(n), JsValue::BigInt(n1)) => JsValue::Number(n + **n1 as f64),
            (JsValue::Str(s1), JsValue::Str(s2)) => concat(s1, s2),
            (JsValue::Str(s), JsValue::BigInt(b)) => concat(s, b),
            (JsValue::Str(s), JsValue::Number(b)) => concat(s, b),
            (JsValue::Str(s), JsValue::Bool(b)) => concat(s, b),
            (JsValue::BigInt(s), JsValue::Str(b)) => concat(s, b),
            (JsValue::Number(s), JsValue::Str(b)) => concat(s, b),
            (JsValue::Bool(s), JsValue::Str(b)) => concat(s, b),
            (_, _) => JsValue::Null,
        }
    }
}

impl From<&Literal> for JsValue {
    fn from(value: &Literal) -> Self {
        match value {
            Literal::Boolean(b) => JsValue::Bool(b.value()),
            Literal::Null(_) => JsValue::Null,
            Literal::Number(n) => match &n.value {
                NumberValue::Float(f) => JsValue::Number(*f),
                NumberValue::Int(i) => JsValue::Number(*i as f64),
            },
            Literal::String(s) => JsValue::Str(s.value.as_str().into()),
        }
    }
}

impl From<Literal> for JsValue {
    fn from(value: Literal) -> Self {
        Self::from(&value)
    }
}

impl From<f64> for JsValue {
    fn from(value: f64) -> Self {
        JsValue::Number(value)
    }
}

impl From<bool> for JsValue {
    fn from(value: bool) -> Self {
        JsValue::Bool(value)
    }
}

impl From<String> for JsValue {
    fn from(value: String) -> Self {
        JsValue::Str(value.into())
    }
}

impl From<&str> for JsValue {
    fn from(value: &str) -> Self {
        JsValue::Str(value.into())
    }
}

impl JsValue {
    pub fn bigint(n: i128) -> Self {
        JsValue::BigInt(Rc::new(n))
    }

    pub fn to_str(&self) -> String {
        format!("{self}")
    }
}

thread_local! {
static PRIMITIVE_PROTO:RcPrototype = Prototype::create(HashMap::from([
    ("valueOf".into(),JsValue::Function("valueOf".into())),
    ("toString".into(),JsValue::Function("toString".into())),
    ("typeOf".into(),JsValue::Function("typeOf".into())),
    ("prototype".into(),JsValue::Undefined)
]),None).into();
}

impl Object for JsValue {
    fn prototype(&self) -> RcPrototype {
        PRIMITIVE_PROTO.with(|v| v.clone())
    }

    fn get_props(&self) -> Vec<String> {
        todo!()
    }

    fn get_methods(&self) -> Vec<String> {
        todo!()
    }

    fn get_prop(&self, prop: &str) -> Result<JsValue, crate::error::JSError> {
        if let Self::Object(obj) = self {
            Ok(obj.get_prop(prop)?)
        } else {
            todo!()
        }
    }

    fn set_prop(&self, prop: &str, value: JsValue) -> Result<(), JSError> {
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
        args: &[Operand],
    ) -> Result<(), JSError> {
        if let Self::Object(ob) = self {
            Ok(ob.call(engine, scope, method, args)?)
        } else if let Self::Function(f) = self {
            let args = args
                .iter()
                .map(|a| engine.resolve_value(a, scope))
                .collect::<Result<Vec<_>, _>>()?;
            let result = engine.call_function(f, args)?;
            engine.set_register(crate::executor::engine::Register::AX, result);
            Ok(())
        } else {
            todo!()
        }
    }

    fn value_of(&self) -> Result<JsValue, JSError> {
        let res = self.to_number();
        match res {
            Ok(f) => Ok(JsValue::Number(f)),
            Err(_) => Ok(JsValue::Null),
        }
    }

    fn type_of(&self) -> String {
        match self {
            JsValue::Number(_) => "Number".into(),
            JsValue::BigInt(_) => "BigInteger".into(),
            JsValue::Str(_) => "String".into(),
            JsValue::Bool(_) => "Boolean".into(),
            JsValue::Null => "(Null)".into(),
            JsValue::Undefined => "(Undefined)".into(),
            JsValue::Symbol(_) => "Symbol".into(),
            JsValue::Object(object) => object.type_of(),
            JsValue::Function(_func) => "Function".into(),
        }
    }
}
