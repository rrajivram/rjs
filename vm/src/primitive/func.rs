use std::{fmt::Display, rc::Rc};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Func(pub Rc<str>);

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
        Self(value.into())
    }
}

impl Display for Func {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)?;
        Ok(())
    }
}
