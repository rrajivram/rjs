use std::{collections::HashMap, rc::Rc};

use crate::{
    error::JSError,
    executor::instr::Operand,
    primitive::{
        JsValue,
        func::Func,
        objects::Object,
        proto::{Prototype, RcPrototype},
    },
};

#[derive(Debug, Clone, Hash)]
pub struct Console {}

const LOG: &str = "log";
const WARN: &str = "warn";
const ERROR: &str = "error";

thread_local! {
    static CONSOLE_PROTO: RcPrototype =Prototype::create(HashMap::from([
        (LOG.into(),JsValue::Function(LOG.into())),
        (WARN.into(),JsValue::Function(WARN.into())),
        (ERROR.into(),JsValue::Function(ERROR.into())),
        ("prototype".into(),JsValue::Undefined)
    ]),None).into();
}

#[allow(clippy::new_without_default)]
impl Console {
    pub fn new() -> Self {
        Self {}
    }
}

impl Object for Console {
    fn prototype(&self) -> RcPrototype {
        CONSOLE_PROTO.with(|p| p.clone())
    }

    fn get_props(&self) -> Vec<String> {
        vec![LOG.into(), WARN.into(), ERROR.into()]
    }

    fn has_prop(&self, prop: &str) -> bool {
        matches!(prop, LOG | WARN | ERROR)
    }

    fn has_method(&self, method: &str) -> bool {
        matches!(method, LOG | WARN | ERROR)
    }

    fn get_methods(&self) -> Vec<String> {
        todo!()
    }

    fn get_prop(&self, prop: &str) -> Result<JsValue, JSError> {
        self.prototype().0.borrow().get(prop)
    }

    fn call(
        &self,
        engine: &mut crate::executor::engine::JSEngine,
        scope: &Rc<crate::executor::scope::Scope>,
        method: &Func,
        args: &[Operand],
    ) -> Result<(), JSError> {
        let pref = match method.as_ref() {
            "log" => "",
            "warn" => "WARN: ",
            "error" => "ERROR: ",
            _ => "UNKNOWN: ",
        };
        print!("{pref}");
        for arg in args {
            let arg = engine.resolve_value(arg, scope)?;
            print!("{arg} ");
        }
        println!();
        Ok(())
    }

    fn type_of(&self) -> String {
        "Console".into()
    }
}

#[cfg(test)]
mod tests {
    use crate::executor::engine::JSEngine;

    use super::*;

    #[test]
    fn test_console() {
        let mut engine = JSEngine::new();
        let scope = engine.scope.clone();
        let var = scope.get_variable("console").unwrap();
        if let JsValue::Object(ob) = &var {
            ob.call(
                &mut engine,
                &scope,
                &Func("warn".into()),
                &[JsValue::from("testing").into()],
            )
            .unwrap();
        }
    }
}
