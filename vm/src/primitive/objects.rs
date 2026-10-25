use std::{cell::RefCell, collections::BTreeMap, fmt::Debug, rc::Rc};

use crate::{
    error::JSError,
    executor::{engine::JSEngine, instr::Value, scope::Scope},
    primitive::{Func, Primitive, RcPrimitive},
};

pub trait Object: Debug {
    fn get_props(&self) -> Vec<String>;
    fn get_methods(&self) -> Vec<String>;
    fn get_prop(&self, prop: &str) -> Result<RcPrimitive, JSError>;
    fn has_prop(&self, prop: &str) -> bool;
    fn has_method(&self, method: &str) -> bool;
    fn call(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        method: &Func,
        args: &[Value],
    ) -> Result<(), JSError>;
    fn value_of(&self) -> Result<Primitive, JSError> {
        Ok(Primitive::Null)
    }
    fn set_prop(&self, _prop: &str, _value: RcPrimitive) -> Result<(), JSError> {
        Ok(())
    }
    fn type_of(&self) -> String;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectPrimitive {
    props: RefCell<BTreeMap<String, RcPrimitive>>,
}

impl ObjectPrimitive {
    pub fn new(props: BTreeMap<String, RcPrimitive>) -> Self {
        Self {
            props: RefCell::new(props),
        }
    }
}

impl Object for ObjectPrimitive {
    fn get_props(&self) -> Vec<String> {
        self.props.borrow().keys().cloned().collect()
    }

    fn get_methods(&self) -> Vec<String> {
        vec![]
    }

    fn has_prop(&self, prop: &str) -> bool {
        self.props.borrow().contains_key(prop)
    }

    fn has_method(&self, _method: &str) -> bool {
        false
    }

    fn call(
        &self,
        _engine: &mut JSEngine,
        _scope: &Rc<Scope>,
        method: &Func,
        _args: &[Value],
    ) -> Result<(), JSError> {
        //let _ = self.get_args(engine)?;
        Err(JSError::MethodNotFound(method.to_string()))
    }

    fn get_prop(&self, prop: &str) -> Result<RcPrimitive, JSError> {
        self.props
            .borrow()
            .get(prop)
            .cloned()
            .ok_or(JSError::ReferenceError(prop.into()))
    }

    fn set_prop(&self, prop: &str, value: RcPrimitive) -> Result<(), JSError> {
        self.props.borrow_mut().insert(prop.to_string(), value);
        Ok(())
    }

    fn type_of(&self) -> String {
        "Object".into()
    }
}
