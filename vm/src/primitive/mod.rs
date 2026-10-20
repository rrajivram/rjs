use std::{cmp::Ordering, default, fmt::Display, hash::Hash, ops::Add, sync::Arc};

use rjs_core::literal::{Literal, NumberLiteral, NumberValue};

use crate::primitive::objects::Object;

pub mod builtin;
pub mod non_numeric;
pub mod numeric;
pub mod objects;

#[derive(Debug, Clone)]
pub enum Primitive {
    Number(f64),
    BigInt(i128),
    Str(String),
    Bool(bool),
    Null,
    Undefined,
    Symbol(u64),
    Object(Object),
    Function,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct ArcPrimitive(pub Arc<Primitive>);

impl Ord for Primitive {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Primitive::Number(n1), Primitive::Number(n2)) => n1.total_cmp(n2),
            (Primitive::Number(n1), Primitive::BigInt(n2)) => (*n1 as i128).cmp(n2),
            (Primitive::BigInt(n1), Primitive::Number(n2)) => n1.cmp(&(*n2 as i128)),
            (Primitive::Str(s1), Primitive::Str(s2)) => s1.cmp(s2),
            (Primitive::Str(_s1), _) => std::cmp::Ordering::Greater,
            (Primitive::Bool(b1), Primitive::Bool(b2)) => b1.cmp(b2),
            (Primitive::Symbol(u1), Primitive::Symbol(u2)) => u1.cmp(u2),
            (Primitive::Object(o1), Primitive::Object(o2)) => o1.cmp(o2),
            (_, _) => std::cmp::Ordering::Less,
        }
    }
}

impl Eq for Primitive {}

impl PartialOrd for Primitive {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Primitive {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Hash for Primitive {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Primitive::Number(n) => n.to_bits().hash(state),
            Primitive::BigInt(n) => n.hash(state),
            Primitive::Str(s) => s.hash(state),
            Primitive::Bool(b) => b.hash(state),
            Primitive::Null => 0.hash(state),
            Primitive::Undefined => 0.hash(state),
            Primitive::Symbol(_) => todo!(),
            Primitive::Object(object) => object.hash(state),
            Primitive::Function => 1.hash(state),
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
            Primitive::Object(_object) => todo!(),
            Primitive::Function => todo!(),
        };
        f.write_str(&s)
    }
}

impl Default for ArcPrimitive {
    fn default() -> Self {
        Self(Arc::new(Primitive::Number(0.)))
    }
}

impl Add for ArcPrimitive {
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

impl From<Primitive> for ArcPrimitive {
    fn from(value: Primitive) -> Self {
        Self(Arc::new(value))
    }
}

impl Display for ArcPrimitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<&Literal> for ArcPrimitive {
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
        Self(Arc::new(val))
    }
}
impl From<Literal> for ArcPrimitive {
    fn from(value: Literal) -> Self {
        Self::from(&value)
    }
}

impl From<f64> for ArcPrimitive {
    fn from(value: f64) -> Self {
        Self(Arc::new(Primitive::Number(value)))
    }
}

impl From<bool> for ArcPrimitive {
    fn from(value: bool) -> Self {
        Self(Arc::new(Primitive::Bool(value)))
    }
}

impl From<String> for ArcPrimitive {
    fn from(value: String) -> Self {
        Self(Arc::new(Primitive::Str(value)))
    }
}

impl From<&str> for ArcPrimitive {
    fn from(value: &str) -> Self {
        Self(Arc::new(Primitive::Str(value.to_string())))
    }
}

impl AsRef<Primitive> for ArcPrimitive {
    fn as_ref(&self) -> &Primitive {
        &self.0
    }
}

impl Primitive {
    pub fn to_str(&self) -> String {
        format!("{self}")
    }
}
