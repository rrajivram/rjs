use std::{
    collections::HashMap,
    sync::{Arc, Weak, atomic::AtomicUsize},
};

use parking_lot::RwLock;
use rjs_core::{keyword::VAR, statement::DeclKind};

use crate::{
    error::JSError,
    primitive::{ArcPrimitive, Primitive},
};

#[derive(Default)]
pub struct Scope {
    pub vars: Arc<RwLock<HashMap<String, Arc<Variable>>>>,
    pub child_scopes: Arc<RwLock<Vec<Arc<Scope>>>>,
    pub parent: Option<Weak<Scope>>,
    pub temp_var: AtomicUsize,
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
    value: Option<ArcPrimitive>,
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
        value: Option<ArcPrimitive>,
        parent: Weak<Scope>,
    ) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            vtype,
            value,
            parent,
        })
    }

    pub fn set(self: &mut Arc<Self>, value: Option<ArcPrimitive>) -> Result<(), JSError> {
        if let VariableType::Const = self.vtype
            && let Some(_v) = &self.value
        {
            return Err(JSError::InvalidReassign(self.name.clone()));
        }
        if let Some(var) = Arc::get_mut(self) {
            var.value = value;
        }
        Ok(())
    }

    pub fn get(self: &mut Arc<Self>, name: &str) -> Result<Option<Arc<Self>>, JSError> {
        todo!()
    }
}

impl Scope {
    pub fn create(parent: Arc<Scope>) -> Arc<Self> {
        let this = Arc::new(Self {
            parent: Some(Arc::downgrade(&parent)),
            ..Default::default()
        });
        parent.child_scopes.write().push(this.clone());
        this
    }

    pub fn new_temp_var(self: &Arc<Self>) -> String {
        let next_var = self
            .temp_var
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let next_var = format!("__t{next_var}");
        let _ = self.add_variable(&next_var, VariableType::Any, None);
        next_var
    }

    pub fn add_variable(
        self: &Arc<Self>,
        name: &str,
        vtype: VariableType,
        value: Option<ArcPrimitive>,
    ) -> Result<(), JSError> {
        let var = Variable::new(name, vtype, value, Arc::downgrade(&self));
        self.vars.write().insert(name.into(), var);
        Ok(())
    }

    pub fn get_variable(self: &Arc<Self>, name: &str) -> Option<ArcPrimitive> {
        if let Some(var) = self.vars.read().get(name)
            && let Some(v) = &var.value
        {
            Some(v.clone())
        } else {
            self.parent
                .as_ref()
                .and_then(|p| p.upgrade().and_then(|p| p.get_variable(name)))
        }
    }

    pub fn set_variable(
        self: &Arc<Self>,
        name: &str,
        value: Option<ArcPrimitive>,
    ) -> Result<(), JSError> {
        if let Some(var) = self.vars.write().get_mut(name) {
            var.set(value)?;
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
        let vars = self.vars.read();
        // HashMap order is random; sort so output is stable between runs.
        let mut names: Vec<_> = vars.keys().collect();
        names.sort();
        for name in names {
            writeln!(f, "{pad}  {}", vars[name])?;
        }
        for child in self.child_scopes.read().iter() {
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
    use std::sync::Arc;

    use crate::{executor::scope::Scope, primitive::ArcPrimitive};

    #[test]
    fn test_scope_var() {
        let scope = Arc::new(Scope::default());
        scope
            .add_variable(
                "test",
                super::VariableType::Any,
                Some(ArcPrimitive::from(1.)),
            )
            .unwrap();
        let v = scope.get_variable("test");
        assert!(v.is_some() && v.as_ref() == Some(&ArcPrimitive::from(1.)));
        assert!(v.is_some() && v.as_ref() != Some(&ArcPrimitive::from(2.)));
        assert!(scope.get_variable("test1").is_none());
    }

    #[test]
    fn test_child_scopes() {
        let scope = Arc::new(Scope::default());
        scope
            .add_variable(
                "test",
                crate::executor::scope::VariableType::Any,
                Some(ArcPrimitive::from(2.0)),
            )
            .unwrap();
        let child = Scope::create(scope.clone());
        child
            .add_variable(
                "test1",
                super::VariableType::Any,
                Some(ArcPrimitive::from(1.0)),
            )
            .unwrap();
        let v = child.get_variable("test1");
        assert!(v.is_some() && v.as_ref() == Some(&ArcPrimitive::from(1.0)));
        let v = child.get_variable("test");
        assert!(v.is_some() && v.as_ref() == Some(&ArcPrimitive::from(2.0)));
        assert!(scope.get_variable("test1").is_none());
    }

    #[test]
    fn test_temp_var() {
        let scope = Arc::new(Scope::default());
        for i in 0..10 {
            assert!(scope.new_temp_var() == format!("__t{i}"))
        }
    }
}
