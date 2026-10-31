use std::{cell::RefCell, collections::HashMap, rc::Rc};

use crate::{
    error::JSError,
    primitive::{JsValue, key::PropertyKey},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prototype {
    pub proto: RefCell<HashMap<PropertyKey, JsValue>>,
    pub parent: Option<RcPrototype>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RcPrototype(pub Rc<RefCell<Prototype>>);

impl Prototype {
    pub fn create(map: HashMap<PropertyKey, JsValue>, parent: Option<RcPrototype>) -> Self {
        Self {
            proto: RefCell::from(map),
            parent,
        }
    }

    pub fn add(&self, key: PropertyKey, value: JsValue) -> Result<(), JSError> {
        let _ = self.proto.borrow_mut().insert(key, value);
        Ok(())
    }

    pub fn get(&self, key: &PropertyKey) -> Result<JsValue, JSError> {
        if let Some(val) = self.proto.borrow().get(key) {
            Ok(val.clone())
        } else {
            if let Some(parent) = &self.parent {
                parent.as_ref().borrow().get(key)
            } else {
                Err(JSError::RefereneError(key.to_string()))
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
