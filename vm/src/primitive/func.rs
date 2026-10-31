use std::{
    collections::HashMap,
    fmt::{Debug, Display},
    hash::Hash,
    ops::Deref,
    rc::{Rc, Weak},
};

use crate::{
    builtin::array::Array,
    error::JSError,
    executor::{
        engine::{JSEngine, Register},
        instr::{Instruction, Operand},
        scope::{Scope, VariableType},
    },
    primitive::{
        JsValue,
        key::PropertyKey,
        objects::{Object, ObjectPrimitive},
        proto::{Prototype, RcPrototype},
    },
};

const FUNC_METHODS: &[&str] = &["call", "apply", "bind", "valueOf", "toString"];

thread_local! {

    static FUNC_PROPS: RcPrototype = Prototype::create(
        FUNC_METHODS
            .iter()
            .map(|m| (PropertyKey::from(*m), JsValue::Function((*m).into())))
            .chain([("prototype".into(), JsValue::Undefined)])
            .collect::<HashMap<_, _>>(),
        Some(ObjectPrimitive::defaul_prototype()),
    )
    .into();

    static FUNC_STAR_PROPS: RcPrototype = Prototype::create(
        FUNC_METHODS
            .iter()
            .map(|m| (PropertyKey::from(*m), JsValue::Function((*m).into())))
            .chain([(PropertyKey::ITERATOR, JsValue::Function("iterator".into()))])
            .chain([("prototype".into(), JsValue::Undefined)])
            .collect::<HashMap<_, _>>(),
        Some(ObjectPrimitive::defaul_prototype()),
    )
    .into();


}

/// A function value: a handle, so copying one (into a register, a
/// variable, an argument) is a reference count, and every copy is the same
/// function.
#[derive(Debug, Clone)]
pub struct Func(Rc<FuncData>);

pub struct FuncData {
    pub name: Rc<str>,
    /// The compiled body of a function written in JS; `None` for a built-in.
    pub instructions: Option<Rc<Vec<Instruction>>>,
    /// The scope the function was created in, which becomes the parent of
    /// each call's scope. Weak so a function stored in its own scope doesn't
    /// form a reference cycle; until closures capture their variables, a
    /// function can't be called once that scope has ended.
    pub scope: Option<Weak<Scope>>,
    pub pc: Option<usize>,
    pub is_generator: bool,
    pub is_async: bool,
    /// The function's `length`: its parameters before the first default or
    /// rest parameter.
    pub arg_count: usize,
    /// For a function made by `bind`: the `this` every call gets, whatever
    /// it's called on.
    pub bound_this: Option<JsValue>,
    /// For a function made by `bind`: arguments passed before the call's own.
    pub bound_args: Vec<JsValue>,
}

impl Deref for Func {
    type Target = FuncData;
    fn deref(&self) -> &FuncData {
        &self.0
    }
}

impl AsRef<str> for Func {
    fn as_ref(&self) -> &str {
        &self.name
    }
}

impl From<&str> for Func {
    fn from(value: &str) -> Self {
        Self(Rc::new(FuncData {
            name: Rc::from(value),
            instructions: None,
            scope: None,
            is_generator: false,
            is_async: false,
            pc: None,
            arg_count: 0,
            bound_this: None,
            bound_args: vec![],
        }))
    }
}

impl From<String> for Func {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

impl Display for Func {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)?;
        Ok(())
    }
}

impl Func {
    /// A function written in JS, not yet created in any scope.
    pub fn compiled(
        name: &str,
        instructions: Vec<Instruction>,
        is_generator: bool,
        is_async: bool,
        arg_count: usize,
    ) -> Self {
        Self(Rc::new(FuncData {
            name: Rc::from(name),
            instructions: Some(Rc::new(instructions)),
            scope: None,
            is_generator,
            is_async,
            pc: Some(0),
            arg_count,
            bound_this: None,
            bound_args: vec![],
        }))
    }

    /// The function value `MakeFunction` creates: this code, created in
    /// `scope`. A new value each time, as each evaluation of a function
    /// declaration or expression is a new function in JS.
    pub fn created_in(&self, scope: &Rc<Scope>) -> Self {
        Self(Rc::new(FuncData {
            name: self.name.clone(),
            instructions: self.instructions.clone(),
            scope: Some(Rc::downgrade(scope)),
            is_async: self.is_async,
            is_generator: self.is_generator,
            pc: self.pc,
            arg_count: self.arg_count,
            bound_this: None,
            bound_args: vec![],
        }))
    }

    /// `self.bind(this, ...args)`: a new function that calls this one with
    /// `this` and `args` in front of its own arguments. Binding a bound
    /// function again only adds arguments; its `this` stays.
    pub fn bound(&self, this: JsValue, args: Vec<JsValue>) -> Self {
        Self(Rc::new(FuncData {
            name: self.name.clone(),
            instructions: self.instructions.clone(),
            scope: self.scope.clone(),
            is_async: self.is_async,
            is_generator: self.is_generator,
            pc: self.pc,
            arg_count: self.arg_count.saturating_sub(args.len()),
            bound_this: Some(self.bound_this.clone().unwrap_or(this)),
            bound_args: self.bound_args.iter().cloned().chain(args).collect(),
        }))
    }

    /// Calls this function with `this` and `args`, leaving the result in AX.
    /// A function written in JS runs on the engine's frame stack, after the
    /// current instruction; a built-in method runs now, on `this`.
    pub fn invoke(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        this: JsValue,
        args: Vec<JsValue>,
    ) -> Result<(), JSError> {
        if self.instructions.is_some() {
            engine.pending_call = Some(self.enter(this, args)?);
            return Ok(());
        }
        let this = self.bound_this.clone().unwrap_or(this);
        if matches!(this, JsValue::Undefined) {
            return Err(JSError::TypeError(format!(
                "{self} must be called as a method"
            )));
        }
        let args: Vec<Operand> = self
            .bound_args
            .iter()
            .cloned()
            .chain(args)
            .map(Operand::Value)
            .collect();
        this.call(engine, scope, self, &args)
    }

    /// Sets up a call to a function written in JS: returns its code and the
    /// scope to run it in, a new scope whose parent is the scope the
    /// function was created in, holding `this` and `arguments`. The engine
    /// runs the code. A bound function's `this` and arguments take effect
    /// here.
    pub fn enter(
        &self,
        this: JsValue,
        args: Vec<JsValue>,
    ) -> Result<(Rc<Vec<Instruction>>, Rc<Scope>), JSError> {
        let Some(code) = &self.instructions else {
            return Err(JSError::MethodNotFound(self.name.to_string()));
        };
        let parent = self.scope.as_ref().and_then(Weak::upgrade).ok_or_else(|| {
            JSError::NotSupported(format!(
                "calling {} after the scope it was created in has ended (closures)",
                self.name
            ))
        })?;
        let scope = Scope::create(parent);
        let this = self.bound_this.clone().unwrap_or(this);
        let args = if self.bound_args.is_empty() {
            args
        } else {
            self.bound_args.iter().cloned().chain(args).collect()
        };
        scope.add_variable("this", VariableType::Const, this)?;
        // The parameters are bound from `arguments` by the function's own code.
        scope.add_variable("arguments", VariableType::Any, Array::create(args))?;
        Ok((code.clone(), scope))
    }
}

impl Hash for Func {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl PartialEq for Func {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Func {}

impl Object for Func {
    fn prototype(&self) -> RcPrototype {
        if self.is_generator {
            FUNC_STAR_PROPS.with(|p| p.clone())
        } else {
            FUNC_PROPS.with(|p| p.clone())
        }
    }

    fn get_prop(&self, prop: &PropertyKey) -> Result<JsValue, JSError> {
        if prop == "length" {
            return Ok((self.arg_count as f64).into());
        }
        self.prototype().0.borrow().get(prop)
    }

    fn call(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        method: &Func,
        args: &[Operand],
    ) -> Result<(), JSError> {
        let mut args = args
            .iter()
            .map(|a| engine.resolve_value(a, scope))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter();
        let name = method.as_ref();
        let res = match name {
            "valueOf" => JsValue::Function(self.clone()),
            "toString" => JsValue::Str("Function code".into()),
            // `f.call(this, ...args)`
            "call" => {
                let this = args.next().unwrap_or(JsValue::Undefined);
                return self.invoke(engine, scope, this, args.collect());
            }
            // `f.apply(this, [args])`
            "apply" => {
                let this = args.next().unwrap_or(JsValue::Undefined);
                let list = match args.next() {
                    None | Some(JsValue::Undefined | JsValue::Null) => vec![],
                    Some(JsValue::Object(o)) if o.as_array().is_some() => {
                        o.as_array().expect("checked above").items()
                    }
                    Some(_) => {
                        return Err(JSError::TypeError(
                            "CreateListFromArrayLike called on non-object".into(),
                        ));
                    }
                };
                return self.invoke(engine, scope, this, list);
            }
            // `f.bind(this, ...args)`
            "bind" => {
                let this = args.next().unwrap_or(JsValue::Undefined);
                JsValue::Function(self.bound(this, args.collect()))
            }
            _ => return Err(JSError::MethodNotFound(name.into())),
        };
        engine.set_register(Register::AX, res);
        Ok(())
    }

    fn type_of(&self) -> String {
        "Function".into()
    }
}

impl Debug for FuncData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FuncData")
            .field("name", &self.name)
            .finish()
    }
}
