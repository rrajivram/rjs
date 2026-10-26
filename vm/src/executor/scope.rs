use std::{
    cell::RefCell,
    collections::HashMap,
    rc::{Rc, Weak},
};

use rjs_core::statement::DeclKind;

use crate::{
    builtin::{array::Array, console::Console},
    error::JSError,
    primitive::{Primitive, RcPrimitive},
};

#[derive(Debug, Default)]
pub struct Scope {
    pub vars: RefCell<HashMap<String, Rc<RefCell<Variable>>>>,
    pub child_scopes: RefCell<Vec<Rc<Scope>>>,
    pub parent: Option<Weak<Scope>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VariableType {
    Let,
    Const,
    Any,
}

#[allow(unused)]
#[derive(Clone, Debug)]
pub struct Variable {
    name: String,
    vtype: VariableType,
    value: RcPrimitive,
    parent: Weak<Scope>,
}

impl Variable {
    pub fn to_string(&self) -> Result<String, JSError> {
        let res = format!("{}", self.value);
        Ok(res)
    }

    pub fn new(
        name: &str,
        vtype: VariableType,
        value: RcPrimitive,
        parent: Weak<Scope>,
    ) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self {
            name: name.into(),
            vtype,
            value,
            parent,
        }))
    }

    pub fn set(&mut self, value: RcPrimitive) -> Result<(), JSError> {
        if let VariableType::Const = self.vtype {
            return Err(JSError::InvalidReassign(self.name.clone()));
        }
        self.value = value;
        Ok(())
    }

    /// Give a declared variable its value at its declaration line. Unlike
    /// `set`, this is allowed for `const`, which is set exactly once this way.
    pub fn init(&mut self, value: RcPrimitive) {
        self.value = value;
    }

    pub fn get(&self, _name: &str) -> Result<Option<Rc<RefCell<Self>>>, JSError> {
        todo!()
    }
}

impl Scope {
    pub fn create_default() -> Rc<Self> {
        let this = Rc::new(Self {
            vars: RefCell::new(HashMap::new()),
            child_scopes: RefCell::new(vec![]),
            parent: None,
        });
        let console = Primitive::Object(Rc::new(Console::new()));
        this.add_variable("console", VariableType::Const, RcPrimitive::from(console))
            .unwrap();
        let array = Primitive::Object(Rc::new(Array::new()));
        this.add_variable("Array", VariableType::Const, array.into())
            .unwrap();
        this
    }

    pub fn create(parent: Rc<Scope>) -> Rc<Self> {
        let this = Rc::new(Self {
            parent: Some(Rc::downgrade(&parent)),
            ..Default::default()
        });
        parent.child_scopes.borrow_mut().push(this.clone());
        this
    }

    pub fn add_variable(
        self: &Rc<Self>,
        name: &str,
        vtype: VariableType,
        value: RcPrimitive,
    ) -> Result<(), JSError> {
        let var = Variable::new(name, vtype, value, Rc::downgrade(self));
        self.vars.borrow_mut().insert(name.into(), var);
        Ok(())
    }

    #[allow(clippy::bind_instead_of_map)]
    pub fn get_variable(self: &Rc<Self>, name: &str) -> Result<RcPrimitive, JSError> {
        if let Some(var) = self.vars.borrow().get(name) {
            Ok(var.borrow().value.clone())
        } else {
            self.parent
                .as_ref()
                .and_then(|p| p.upgrade().and_then(|p| Some(p.get_variable(name))))
                .transpose()?
                .ok_or(JSError::ReferenceError(name.into()))
        }
    }

    /// The nearest variable called `name`, searching outward from this scope.
    fn find_variable(self: &Rc<Self>, name: &str) -> Option<Rc<RefCell<Variable>>> {
        // Clone the handle so the map borrow ends before anyone mutates it.
        if let Some(var) = self.vars.borrow().get(name).cloned() {
            return Some(var);
        }
        self.parent.as_ref()?.upgrade()?.find_variable(name)
    }

    pub fn set_variable(self: &Rc<Self>, name: &str, value: RcPrimitive) -> Result<(), JSError> {
        self.find_variable(name)
            .ok_or_else(|| JSError::ObjectNotFound(name.into()))?
            .borrow_mut()
            .set(value)
    }

    /// Run a declaration's initializer: like `set_variable`, but allowed for
    /// `const`.
    pub fn init_variable(self: &Rc<Self>, name: &str, value: RcPrimitive) -> Result<(), JSError> {
        self.find_variable(name)
            .ok_or_else(|| JSError::ObjectNotFound(name.into()))?
            .borrow_mut()
            .init(value);
        Ok(())
    }

    /// Whether this scope itself (not a parent) declares `name`.
    pub fn has_own_variable(&self, name: &str) -> bool {
        self.vars.borrow().contains_key(name)
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
        write!(f, "{}", self.value)
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

    use crate::{error::JSError, executor::scope::Scope, primitive::RcPrimitive};

    #[test]
    fn test_scope_var() {
        let scope = Rc::new(Scope::default());
        scope
            .add_variable("test", super::VariableType::Any, RcPrimitive::from(1.))
            .unwrap();
        let v = scope.get_variable("test").unwrap();
        assert!(v == RcPrimitive::from(1.));
        assert!(v != RcPrimitive::from(2.));
        assert!(matches!(
            scope.get_variable("test1"),
            Err(JSError::ReferenceError(_))
        ));
    }

    #[test]
    fn test_child_scopes() {
        let scope = Rc::new(Scope::default());
        scope
            .add_variable(
                "test",
                crate::executor::scope::VariableType::Any,
                RcPrimitive::from(2.0),
            )
            .unwrap();
        let child = Scope::create(scope.clone());
        child
            .add_variable("test1", super::VariableType::Any, RcPrimitive::from(1.0))
            .unwrap();
        assert!(child.get_variable("test1").unwrap() == RcPrimitive::from(1.0));
        assert!(child.get_variable("test").unwrap() == RcPrimitive::from(2.0));
        assert!(matches!(
            scope.get_variable("test1"),
            Err(JSError::ReferenceError(_))
        ));
    }

    #[test]
    fn test_set_variable_while_shared() {
        let scope = Rc::new(Scope::default());
        scope
            .add_variable("x", super::VariableType::Let, RcPrimitive::from(1.0))
            .unwrap();
        // A second handle to the variable, as a closure capturing it would hold.
        let _shared = scope.vars.borrow()["x"].clone();
        scope.set_variable("x", RcPrimitive::from(2.0)).unwrap();
        assert_eq!(scope.get_variable("x").unwrap(), RcPrimitive::from(2.0));
    }
}
