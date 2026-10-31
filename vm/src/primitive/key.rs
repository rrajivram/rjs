//! Property keys. A JS property key is a string or a symbol; any other value
//! used as a key becomes its string form (`a[1]` is the property "1").

use std::{fmt::Display, rc::Rc};

use crate::{error::JSError, primitive::JsValue};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PropertyKey {
    Str(Rc<str>),
    /// The same ids as `JsValue::Symbol`. Ids below
    /// `PropertyKey::FIRST_PROGRAM_SYMBOL` are the well-known symbols.
    Symbol(u64),
}

impl PropertyKey {
    /// The id of `Symbol.iterator`, for `JsValue::Symbol`.
    pub const ITERATOR_SYMBOL_VAL: u64 = 1;
    /// The id of `Symbol.asyncIterator`, for `JsValue::Symbol`.
    pub const ASYNC_ITERATOR_SYMBOL_VAL: u64 = 2;
    /// `Symbol.iterator`: the method `for-of`, spread and destructuring call
    /// to get an iterator.
    pub const ITERATOR: Self = Self::Symbol(Self::ITERATOR_SYMBOL_VAL);
    /// `Symbol.asyncIterator`: the method `for await` calls.
    pub const ASYNC_ITERATOR: Self = Self::Symbol(Self::ASYNC_ITERATOR_SYMBOL_VAL);
    /// The first symbol id for symbols a program creates (`Symbol()`).
    pub const FIRST_PROGRAM_SYMBOL: u64 = 64;

    /// ToPropertyKey: a symbol stays a symbol, anything else becomes its
    /// string form.
    pub fn from_value(value: &JsValue) -> Result<Self, JSError> {
        Ok(match value {
            JsValue::Symbol(id) => Self::Symbol(*id),
            JsValue::Str(s) => Self::Str(s.clone()),
            other => Self::Str(other.to_js_string()?.into()),
        })
    }

    /// The key's string, or `None` for a symbol.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            Self::Symbol(_) => None,
        }
    }
}

impl From<&str> for PropertyKey {
    fn from(value: &str) -> Self {
        Self::Str(value.into())
    }
}

impl From<String> for PropertyKey {
    fn from(value: String) -> Self {
        Self::Str(value.into())
    }
}

/// So a key can be compared with a name directly: `key == "length"`.
impl PartialEq<str> for PropertyKey {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == Some(other)
    }
}

impl Display for PropertyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Str(s) => f.write_str(s),
            Self::Symbol(_) if *self == Self::ITERATOR => f.write_str("Symbol(Symbol.iterator)"),
            Self::Symbol(_) if *self == Self::ASYNC_ITERATOR => {
                f.write_str("Symbol(Symbol.asyncIterator)")
            }
            Self::Symbol(_) => f.write_str("Symbol()"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PropertyKey;
    use crate::primitive::JsValue;

    #[test]
    fn values_become_keys() {
        let key = |v: JsValue| PropertyKey::from_value(&v).unwrap();
        assert_eq!(key("a".into()), PropertyKey::from("a"));
        assert_eq!(key(1.0.into()), PropertyKey::from("1"));
        assert_eq!(key(1.5.into()), PropertyKey::from("1.5"));
        assert_eq!(key(true.into()), PropertyKey::from("true"));
        assert_eq!(key(JsValue::Undefined), PropertyKey::from("undefined"));
        assert_eq!(key(JsValue::Symbol(1)), PropertyKey::ITERATOR);
        // A symbol is never equal to a string, even one that prints the same.
        assert_ne!(
            PropertyKey::ITERATOR,
            PropertyKey::from("Symbol(Symbol.iterator)")
        );
        let length = PropertyKey::from("length");
        assert!(length == *"length" && PropertyKey::ITERATOR != *"length");
        assert_eq!(PropertyKey::ITERATOR.to_string(), "Symbol(Symbol.iterator)");
    }
}
