use std::rc::Rc;

use crate::{
    executor::instr::Value,
    primitive::{Func, Primitive, RcPrimitive, objects::Object},
};

#[derive(Debug, Clone, Hash)]
pub struct Console {
    methods: Vec<String>,
    props: Vec<RcPrimitive>,
}

const LOG: &str = "log";
const WARN: &str = "warn";
const ERROR: &str = "error";

#[allow(clippy::new_without_default)]
impl Console {
    pub fn new() -> Self {
        Self {
            methods: Vec::from([LOG.to_string(), WARN.into(), ERROR.into()]),
            props: vec![
                RcPrimitive::from(Primitive::Function(LOG.into())),
                RcPrimitive::from(Primitive::Function(WARN.into())),
                RcPrimitive::from(Primitive::Function(ERROR.into())),
            ],
        }
    }
}

impl Object for Console {
    fn get_props(&self) -> Vec<String> {
        vec![LOG.into(), WARN.into(), ERROR.into()]
    }

    fn has_prop(&self, prop: &str) -> bool {
        matches!(prop, LOG | WARN | ERROR)
    }

    fn has_method(&self, method: &str) -> bool {
        self.methods.contains(&method.to_string())
    }

    fn get_methods(&self) -> Vec<String> {
        self.methods.clone()
    }

    fn get_prop(&self, prop: &str) -> Result<crate::primitive::RcPrimitive, crate::error::JSError> {
        match prop {
            LOG => Ok(self.props[0].clone()),
            WARN => Ok(self.props[1].clone()),
            ERROR => Ok(self.props[2].clone()),
            _ => Err(crate::error::JSError::RefereneError(prop.into())),
        }
    }

    fn call(
        &self,
        engine: &mut crate::executor::engine::JSEngine,
        scope: &Rc<crate::executor::scope::Scope>,
        method: &Func,
        args: &[Value],
    ) -> Result<(), crate::error::JSError> {
        let pref = match method.as_ref() {
            "log" => "",
            "warn" => "WARN: ",
            "error" => "ERROR: ",
            _ => "UNKNOWN: ",
        };
        print!("{pref}");
        for arg in args {
            let arg = engine.resolve_value(arg, scope)?;
            print!("{arg}");
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
        if let Primitive::Object(ob) = var.as_ref() {
            ob.call(
                &mut engine,
                &scope,
                &Func("warn".into()),
                &[RcPrimitive::from("testing").into()],
            )
            .unwrap();
        }
    }
}
