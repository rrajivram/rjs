use std::{cmp::Ordering, hash::Hash};

use crate::primitive::objects::Object;

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
