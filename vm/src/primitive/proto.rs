use std::{cell::RefCell, collections::HashMap, rc::Rc};

use crate::{error::JSError, primitive::JsValue};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prototype {
    pub proto: RefCell<HashMap<String, JsValue>>,
    pub parent: Option<RcPrototype>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RcPrototype(pub Rc<RefCell<Prototype>>);

impl Prototype {
    pub fn create(map: HashMap<String, JsValue>, parent: Option<RcPrototype>) -> Self {
        Self {
            proto: RefCell::from(map),
            parent,
        }
    }

    pub fn add(&self, name: &str, value: JsValue) -> Result<(), JSError> {
        let _ = self.proto.borrow_mut().insert(name.to_string(), value);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<JsValue, JSError> {
        if let Some(val) = self.proto.borrow().get(name) {
            Ok(val.clone())
        } else {
            if let Some(parent) = &self.parent {
                parent.as_ref().borrow().get(name)
            } else {
                Err(JSError::RefereneError(name.into()))
            }
        }
    }
}

impl From<Prototype> for RcPrototype {
    fn from(value: Prototype) -> Self {
        Self(Rc::new(RefCell::new(value)))
    }
}

impl AsRef<Rc<RefCell<Prototype>>> for RcPrototype {
    fn as_ref(&self) -> &Rc<RefCell<Prototype>> {
        &self.0
    }
}
