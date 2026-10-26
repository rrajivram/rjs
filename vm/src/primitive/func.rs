use std::fmt::Display;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Func(pub String);

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
