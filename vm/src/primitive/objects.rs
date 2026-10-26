use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    fmt::Debug,
    rc::Rc,
};

use crate::{
    builtin::array::Array,
    error::JSError,
    executor::{engine::JSEngine, instr::Value, scope::Scope},
    primitive::{
        Func, Primitive, RcPrimitive,
        proto::{Prototype, RcPrototype},
    },
};

pub trait Object: Debug {
    fn prototype(&self) -> RcPrototype;
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
    /// The array behind this object, if it is one.
    fn as_array(&self) -> Option<&Array> {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectPrimitive {
    prototype: RcPrototype,
    props: RefCell<BTreeMap<String, RcPrimitive>>,
}

thread_local! {
    static OBJ_PROTO: RcPrototype = Prototype::create(HashMap::from([
        ("valueOf".into(),RcPrimitive::from(Primitive::Function("valueOf".into()))),
        ("toString".into(),RcPrimitive::from(Primitive::Function("toString".into()))),
        ("typeOf".into(),RcPrimitive::from(Primitive::Function("typeOf".into()))),
        ("prototype".into(),RcPrimitive::from(Primitive::Undefined))
    ]),None).into();
}

impl ObjectPrimitive {
    pub fn new(props: BTreeMap<String, RcPrimitive>) -> Self {
        let prototype = RcPrototype::from(Prototype::create(
            HashMap::from([
                (
                    "valueOf".into(),
                    RcPrimitive::from(Primitive::Function("valueOf".into())),
                ),
                (
                    "toString".into(),
                    RcPrimitive::from(Primitive::Function("toString".into())),
                ),
                (
                    "typeOf".into(),
                    RcPrimitive::from(Primitive::Function("typeOf".into())),
                ),
                ("prototype".into(), RcPrimitive::from(Primitive::Undefined)),
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
