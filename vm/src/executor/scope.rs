use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
};

use rjs_core::{keyword::VAR, statement::DeclKind};

use crate::{
    error::JSError,
    primitive::{RcPrimitive, Primitive},
};

#[derive(Default)]
pub struct Scope {
    pub vars: RefCell<HashMap<String, Rc<RefCell<Variable>>>>,
    pub child_scopes: RefCell<Vec<Rc<Scope>>>,
    pub parent: Option<Weak<Scope>>,
    pub temp_var: Cell<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VariableType {
    Let,
    Const,
    Any,
}

#[derive(Clone)]
pub struct Variable {
    name: String,
    vtype: VariableType,
    value: Option<RcPrimitive>,
    parent: Weak<Scope>,
}

impl Variable {
    pub fn to_string(&self) -> Result<String, JSError> {
        let res = match &self.value {
            Some(v) => {
                format!("{}", v)
            }
            None => match self.vtype {
                VariableType::Const => return Err(JSError::RefereneError(self.name.clone())),
                _ => "undefined".into(),
            },
        };
        Ok(res)
    }

    pub fn new(
        name: &str,
        vtype: VariableType,
        value: Option<RcPrimitive>,
        parent: Weak<Scope>,
    ) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self {
            name: name.into(),
            vtype,
            value,
            parent,
        }))
    }

    pub fn set(&mut self, value: Option<RcPrimitive>) -> Result<(), JSError> {
        if let VariableType::Const = self.vtype
            && let Some(_v) = &self.value
        {
            return Err(JSError::InvalidReassign(self.name.clone()));
        }
        self.value = value;
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<Option<Rc<RefCell<Self>>>, JSError> {
        todo!()
    }
}

impl Scope {
    pub fn create(parent: Rc<Scope>) -> Rc<Self> {
        let this = Rc::new(Self {
            parent: Some(Rc::downgrade(&parent)),
            ..Default::default()
        });
        parent.child_scopes.borrow_mut().push(this.clone());
        this
    }

    pub fn new_temp_var(self: &Rc<Self>) -> String {
        let next_var = self.temp_var.get();
        self.temp_var.set(next_var + 1);
        let next_var = format!("__t{next_var}");
        let _ = self.add_variable(&next_var, VariableType::Any, None);
        next_var
    }

    pub fn add_variable(
        self: &Rc<Self>,
        name: &str,
        vtype: VariableType,
        value: Option<RcPrimitive>,
    ) -> Result<(), JSError> {
        let var = Variable::new(name, vtype, value, Rc::downgrade(self));
        self.vars.borrow_mut().insert(name.into(), var);
        Ok(())
    }

    pub fn get_variable(self: &Rc<Self>, name: &str) -> Option<RcPrimitive> {
        if let Some(var) = self.vars.borrow().get(name)
            && let Some(v) = &var.borrow().value
        {
            Some(v.clone())
        } else {
            self.parent
                .as_ref()
                .and_then(|p| p.upgrade().and_then(|p| p.get_variable(name)))
        }
    }

    pub fn set_variable(
        self: &Rc<Self>,
        name: &str,
        value: Option<RcPrimitive>,
    ) -> Result<(), JSError> {
        // Release the map borrow before mutating the variable.
        let var = self.vars.borrow().get(name).cloned();
        if let Some(var) = var {
            var.borrow_mut().set(value)?;
        } else if let Some(p) = self.parent.as_ref()
            && let Some(parent) = p.upgrade()
        {
            parent.set_variable(name, value)?;
        } else {
            return Err(JSError::ObjectNotFound(name.into()));
        }

        Ok(())
    }
}

impl From<DeclKind> for VariableType {
    fn from(value: DeclKind) -> Self {
        match value {
            DeclKind::Const(_) => Self::Const,
            DeclKind::Var(_) => Self::Any,
            DeclKind::Let(_) => Self::Let,
        }
    }
}

impl std::fmt::Display for VariableType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            VariableType::Let => "let",
            VariableType::Const => "const",
            VariableType::Any => "var",
        })
    }
}

impl std::fmt::Display for Variable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {} = ", self.vtype, self.name)?;
        match &self.value {
            Some(v) => write!(f, "{v}"),
            None => f.write_str("<uninitialized>"),
        }
    }
}

impl Scope {
    fn fmt_tree(&self, f: &mut std::fmt::Formatter<'_>, depth: usize) -> std::fmt::Result {
        let pad = "  ".repeat(depth);
        writeln!(f, "{pad}scope {{")?;
        let vars = self.vars.borrow();
        // HashMap order is random; sort so output is stable between runs.
        let mut names: Vec<_> = vars.keys().collect();
        names.sort();
        for name in names {
            writeln!(f, "{pad}  {}", vars[name].borrow())?;
        }
        for child in self.child_scopes.borrow().iter() {
            child.fmt_tree(f, depth + 1)?;
        }
        writeln!(f, "{pad}}}")
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.fmt_tree(f, 0)
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::{executor::scope::Scope, primitive::RcPrimitive};

    #[test]
    fn test_scope_var() {
        let scope = Rc::new(Scope::default());
        scope
            .add_variable(
                "test",
                super::VariableType::Any,
                Some(RcPrimitive::from(1.)),
            )
            .unwrap();
        let v = scope.get_variable("test");
        assert!(v.is_some() && v.as_ref() == Some(&RcPrimitive::from(1.)));
        assert!(v.is_some() && v.as_ref() != Some(&RcPrimitive::from(2.)));
        assert!(scope.get_variable("test1").is_none());
    }

    #[test]
    fn test_child_scopes() {
        let scope = Rc::new(Scope::default());
        scope
            .add_variable(
                "test",
                crate::executor::scope::VariableType::Any,
                Some(RcPrimitive::from(2.0)),
            )
            .unwrap();
        let child = Scope::create(scope.clone());
        child
            .add_variable(
                "test1",
                super::VariableType::Any,
                Some(RcPrimitive::from(1.0)),
            )
            .unwrap();
        let v = child.get_variable("test1");
        assert!(v.is_some() && v.as_ref() == Some(&RcPrimitive::from(1.0)));
        let v = child.get_variable("test");
        assert!(v.is_some() && v.as_ref() == Some(&RcPrimitive::from(2.0)));
        assert!(scope.get_variable("test1").is_none());
    }

    #[test]
    fn test_set_variable_while_shared() {
        let scope = Rc::new(Scope::default());
        scope
            .add_variable("x", super::VariableType::Let, Some(RcPrimitive::from(1.0)))
            .unwrap();
        // A second handle to the variable, as a closure capturing it would hold.
        let _shared = scope.vars.borrow()["x"].clone();
        scope.set_variable("x", Some(RcPrimitive::from(2.0))).unwrap();
        assert_eq!(scope.get_variable("x"), Some(RcPrimitive::from(2.0)));
    }

    #[test]
    fn test_temp_var() {
        let scope = Rc::new(Scope::default());
        for i in 0..10 {
            assert!(scope.new_temp_var() == format!("__t{i}"))
        }
    }
}
