use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    fmt::Debug,
    rc::Rc,
};

use crate::{
    builtin::array::Array,
    error::JSError,
    executor::{engine::JSEngine, instr::Operand, scope::Scope},
    primitive::{
        Func, JsValue,
        key::PropertyKey,
        proto::{Prototype, RcPrototype},
    },
};

pub trait Object: Debug {
    fn prototype(&self) -> RcPrototype;
    fn get_props(&self) -> Vec<PropertyKey> {
        vec![]
    }
    fn get_methods(&self) -> Vec<String> {
        vec![]
    }
    fn get_prop(&self, _prop: &PropertyKey) -> Result<JsValue, JSError>;
    fn has_prop(&self, _prop: &PropertyKey) -> bool {
        false
    }
    fn has_method(&self, _method: &str) -> bool {
        false
    }
    fn call(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        method: &Func,
        args: &[Operand],
    ) -> Result<(), JSError>;
    fn value_of(&self) -> Result<JsValue, JSError> {
        Ok(JsValue::Null)
    }
    fn set_prop(&self, _prop: &PropertyKey, _value: JsValue) -> Result<(), JSError> {
        Ok(())
    }
    fn type_of(&self) -> String;
    /// Calling the object itself, `obj(...)`, for built-ins that can be
    /// called as well as having properties (`Symbol()`). The result goes in
    /// AX.
    fn call_as_function(
        &self,
        _engine: &mut JSEngine,
        _scope: &Rc<Scope>,
        _args: &[Operand],
    ) -> Result<(), JSError> {
        Err(JSError::TypeError(format!("{} is not a function", self.type_of())))
    }
    /// The array behind this object, if it is one.
    fn as_array(&self) -> Option<&Array> {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectPrimitive {
    prototype: RcPrototype,
    props: RefCell<BTreeMap<PropertyKey, JsValue>>,
}

thread_local! {
    static OBJ_PROTO: RcPrototype = Prototype::create(HashMap::from([
        ("valueOf".into(),JsValue::Function("valueOf".into())),
        ("toString".into(),JsValue::Function("toString".into())),
        ("typeOf".into(),JsValue::Function("typeOf".into())),
        ("prototype".into(),JsValue::Undefined)
    ]),None).into();
}

impl ObjectPrimitive {
    pub fn new(props: BTreeMap<PropertyKey, JsValue>) -> Self {
        let prototype = RcPrototype::from(Prototype::create(
            HashMap::from([
                ("valueOf".into(), JsValue::Function("valueOf".into())),
                ("toString".into(), JsValue::Function("toString".into())),
                ("typeOf".into(), JsValue::Function("typeOf".into())),
                ("prototype".into(), JsValue::Undefined),
            ]),
            None,
        ));
        Self {
            props: RefCell::new(props),
            prototype,
        }
    }

    pub fn defaul_prototype() -> RcPrototype {
        OBJ_PROTO.with(|p| p.clone())
    }
}

impl Object for ObjectPrimitive {
    fn prototype(&self) -> RcPrototype {
        self.prototype.clone()
    }

    fn get_props(&self) -> Vec<PropertyKey> {
        self.props.borrow().keys().cloned().collect()
    }

    fn get_methods(&self) -> Vec<String> {
        vec![]
    }

    fn has_prop(&self, prop: &PropertyKey) -> bool {
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
        _args: &[Operand],
    ) -> Result<(), JSError> {
        //let _ = self.get_args(engine)?;
        Err(JSError::MethodNotFound(method.to_string()))
    }

    fn get_prop(&self, prop: &PropertyKey) -> Result<JsValue, JSError> {
        self.props
            .borrow()
            .get(prop)
            .cloned()
            .ok_or_else(|| JSError::ReferenceError(prop.to_string()))
    }

    fn set_prop(&self, prop: &PropertyKey, value: JsValue) -> Result<(), JSError> {
        self.props.borrow_mut().insert(prop.clone(), value);
        Ok(())
    }

    fn type_of(&self) -> String {
        "Object".into()
    }
}
