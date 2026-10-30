use std::{
    fmt::Display,
    hash::Hash,
    ops::Deref,
    rc::{Rc, Weak},
};

use crate::{
    builtin::array::Array,
    error::JSError,
    executor::{
        instr::Instruction,
        scope::{Scope, VariableType},
    },
    primitive::JsValue,
};

/// A function value: a handle, so copying one (into a register, a
/// variable, an argument) is a reference count, and every copy is the same
/// function.
#[derive(Debug, Clone)]
pub struct Func(Rc<FuncData>);

#[derive(Debug)]
pub struct FuncData {
    pub name: Rc<str>,
    /// The compiled body of a function written in JS; `None` for a built-in.
    pub instructions: Option<Rc<Vec<Instruction>>>,
    /// The scope the function was created in, which becomes the parent of
    /// each call's scope. Weak so a function stored in its own scope doesn't
    /// form a reference cycle; until closures capture their variables, a
    /// function can't be called once that scope has ended.
    pub scope: Option<Weak<Scope>>,
}

impl Deref for Func {
    type Target = FuncData;
    fn deref(&self) -> &FuncData {
        &self.0
    }
}

impl AsRef<str> for Func {
    fn as_ref(&self) -> &str {
        &self.name
    }
}

impl From<&str> for Func {
    fn from(value: &str) -> Self {
        Self(Rc::new(FuncData {
            name: Rc::from(value),
            instructions: None,
            scope: None,
        }))
    }
}

impl From<String> for Func {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

impl Display for Func {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)?;
        Ok(())
    }
}

impl Func {
    /// A function written in JS, not yet created in any scope.
    pub fn compiled(name: &str, instructions: Vec<Instruction>) -> Self {
        Self(Rc::new(FuncData {
            name: Rc::from(name),
            instructions: Some(Rc::new(instructions)),
            scope: None,
        }))
    }

    /// The function value `MakeFunction` creates: this code, created in
    /// `scope`. A new value each time, as each evaluation of a function
    /// declaration or expression is a new function in JS.
    pub fn created_in(&self, scope: &Rc<Scope>) -> Self {
        Self(Rc::new(FuncData {
            name: self.name.clone(),
            instructions: self.instructions.clone(),
            scope: Some(Rc::downgrade(scope)),
        }))
    }

    /// Sets up a call to a function written in JS: returns its code and the
    /// scope to run it in, a new scope whose parent is the scope the
    /// function was created in. The engine runs the code.
    pub fn enter(
        &self,
        args: Vec<JsValue>,
    ) -> Result<(Rc<Vec<Instruction>>, Rc<Scope>), JSError> {
        let Some(code) = &self.instructions else {
            return Err(JSError::MethodNotFound(self.name.to_string()));
        };
        let parent = self
            .scope
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| {
                JSError::NotSupported(format!(
                    "calling {} after the scope it was created in has ended (closures)",
                    self.name
                ))
            })?;
        let scope = Scope::create(parent);
        // The parameters are bound from `arguments` by the function's own code.
        scope.add_variable("arguments", VariableType::Any, Array::create(args))?;
        Ok((code.clone(), scope))
    }
}

impl Hash for Func {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl PartialEq for Func {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Func {}
