use std::{cell::Cell, collections::HashMap, rc::Rc};

use crate::{
    error::JSError,
    executor::{
        engine::{JSEngine, Register},
        instr::Operand,
        scope::Scope,
    },
    primitive::{
        JsValue,
        func::Func,
        key::PropertyKey,
        objects::{Object, ObjectPrimitive},
        proto::{Prototype, RcPrototype},
    },
};

/// The global `Symbol`: called as `Symbol()` to make a new symbol, and
/// holding the well-known symbols (`Symbol.iterator`, ...).
#[derive(Debug, Clone, Default)]
pub struct SymbolObject {}

const SYMBOL_METHODS: &[&str] = &["new", "valueOf", "toString"];

thread_local! {

    static SYMBOL_PROPS: RcPrototype = Prototype::create(
        SYMBOL_METHODS
            .iter()
            .map(|m| (PropertyKey::from(*m), JsValue::Function((*m).into())))
            .chain([("iterator".into(),JsValue::Symbol(PropertyKey::ITERATOR_SYMBOL_VAL))])
            .chain([(
                "asyncIterator".into(),
                JsValue::Symbol(PropertyKey::ASYNC_ITERATOR_SYMBOL_VAL),
            )])
            .chain([("prototype".into(), JsValue::Undefined)])
            .collect::<HashMap<_, _>>(),
        Some(ObjectPrimitive::defaul_prototype()),
    )
    .into();

    /// The id the next `Symbol()` gets.
    static NEXT_SYMBOL: Cell<u64> = const { Cell::new(PropertyKey::FIRST_PROGRAM_SYMBOL) };

}

impl SymbolObject {
    /// A symbol no other symbol equals. Descriptions (`Symbol('name')`)
    /// aren't kept: the value is only an id.
    pub fn new_symbol() -> JsValue {
        NEXT_SYMBOL.with(|next| {
            let id = next.get();
            next.set(id + 1);
            JsValue::Symbol(id)
        })
    }
}

impl Object for SymbolObject {
    fn prototype(&self) -> RcPrototype {
        SYMBOL_PROPS.with(|p| p.clone())
    }

    fn get_prop(&self, prop: &PropertyKey) -> Result<JsValue, JSError> {
        Ok(self
            .prototype()
            .0
            .borrow()
            .get(prop)
            .unwrap_or(JsValue::Undefined))
    }

    fn has_prop(&self, prop: &PropertyKey) -> bool {
        self.prototype().0.borrow().get(prop).is_ok()
    }

    fn call(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        method: &Func,
        args: &[Operand],
    ) -> Result<(), JSError> {
        let result = match method.as_ref() {
            "new" => return self.call_as_function(engine, scope, args),
            "toString" => "function Symbol() { [native code] }".into(),
            "valueOf" => {
                return Err(JSError::NotSupported(
                    "Symbol.valueOf(): the global Symbol can't refer to itself yet".into(),
                ));
            }
            name => return Err(JSError::MethodNotFound(name.into())),
        };
        engine.set_register(Register::AX, result);
        Ok(())
    }

    /// `Symbol()` or `Symbol(description)`.
    fn call_as_function(
        &self,
        engine: &mut JSEngine,
        _scope: &Rc<Scope>,
        _args: &[Operand],
    ) -> Result<(), JSError> {
        engine.set_register(Register::AX, Self::new_symbol());
        Ok(())
    }

    fn type_of(&self) -> String {
        "Symbol".into()
    }
}
