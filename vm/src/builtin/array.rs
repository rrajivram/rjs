use std::{
    cell::RefCell,
    collections::HashMap,
    rc::{Rc, Weak},
};

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
        objects::{Object, ObjectPrimitive},
        proto::{Prototype, RcPrototype},
    },
};

/// Methods implemented by [`Array::call_func`]. `new`, `isArray`, `of` and
/// `from` are reached through the global `Array` object.
const METHODS: &[&str] = &[
    "new",
    "isArray",
    "of",
    "from",
    "at",
    "concat",
    "copyWithin",
    "fill",
    "flat",
    "includes",
    "indexOf",
    "join",
    "lastIndexOf",
    "pop",
    "push",
    "reverse",
    "shift",
    "slice",
    "sort",
    "splice",
    "toReversed",
    "toSorted",
    "toSpliced",
    "toString",
    "unshift",
    "with",
];

/// Methods that take a callback. The VM can't call JS functions yet, so
/// these report that instead of failing as unknown properties.
const CALLBACK_METHODS: &[&str] = &[
    "every",
    "filter",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "flatMap",
    "forEach",
    "map",
    "reduce",
    "reduceRight",
    "some",
];

/// Methods that return iterators, which the VM doesn't have yet.
const ITERATOR_METHODS: &[&str] = &["entries", "keys", "values"];

thread_local! {
    static PROPS: RcPrototype = Prototype::create(
        METHODS
            .iter()
            .chain(CALLBACK_METHODS)
            .chain(ITERATOR_METHODS)
            .map(|m| (m.to_string(), JsValue::Function((*m).into())))
            .chain([("prototype".to_string(), JsValue::Undefined)])
            .collect::<HashMap<_, _>>(),
        Some(ObjectPrimitive::defaul_prototype()),
    )
    .into();

    /// Arrays currently being joined, so a cycle (`a.push(a)`) joins as ""
    /// instead of recursing forever.
    static JOINING: RefCell<Vec<*const Array>> = const { RefCell::new(Vec::new()) };
}

#[derive(Debug, Clone, Default)]
pub struct Array {
    items: RefCell<Vec<JsValue>>,
    /// The `Rc` this array lives in, so methods like `reverse` can return
    /// the array itself. Empty for arrays not built with [`Array::create`].
    this: Weak<Array>,
}

impl Array {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(items: &[JsValue]) -> Self {
        Self {
            items: RefCell::from(items.to_vec()),
            this: Weak::new(),
        }
    }

    /// A new array value.
    pub fn create(items: Vec<JsValue>) -> JsValue {
        let array = Rc::new_cyclic(|this| Self {
            items: RefCell::new(items),
            this: this.clone(),
        });
        JsValue::Object(array)
    }

    pub fn len(&self) -> usize {
        self.items.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn items(&self) -> Vec<JsValue> {
        self.items.borrow().clone()
    }

    /// The array as a value, for methods that return `this`.
    fn this(&self) -> Result<JsValue, JSError> {
        self.this
            .upgrade()
            .map(|array| JsValue::Object(array))
            .ok_or_else(|| JSError::UnknownError("array has no reference to itself".into()))
    }

    pub fn call_func(
        &self,
        func: &Func,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        args: &[Operand],
    ) -> Result<(), JSError> {
        let name = func.as_ref();
        if name == "new" {
            return self.do_new(engine, scope, args);
        }
        if CALLBACK_METHODS.contains(&name) {
            return Err(JSError::NotSupported(format!(
                "Array.prototype.{name} takes a callback, and the VM can't call JS functions yet"
            )));
        }
        if ITERATOR_METHODS.contains(&name) {
            return Err(JSError::NotSupported(format!(
                "Array.prototype.{name} returns an iterator, which the VM doesn't support yet"
            )));
        }
        let args = args
            .iter()
            .map(|a| engine.resolve_value(a, scope))
            .collect::<Result<Vec<_>, _>>()?;
        let result = match name {
            "isArray" => Self::do_is_array(&args),
            "of" => Ok(Self::create(args)),
            "from" => Self::do_from(&args),
            "at" => self.do_at(&args),
            "concat" => self.do_concat(&args),
            "copyWithin" => self.do_copy_within(&args),
            "fill" => self.do_fill(&args),
            "flat" => self.do_flat(&args),
            "includes" => self.do_includes(&args),
            "indexOf" => self.do_index_of(&args),
            "join" => self.do_join(&args),
            "lastIndexOf" => self.do_last_index_of(&args),
            "pop" => Ok(self.items.borrow_mut().pop().unwrap_or(JsValue::Undefined)),
            "push" => self.do_push(args),
            "reverse" => {
                self.items.borrow_mut().reverse();
                self.this()
            }
            "shift" => self.do_shift(),
            "slice" => self.do_slice(&args),
            "sort" => {
                let sorted = sort_items(self.items(), &args)?;
                *self.items.borrow_mut() = sorted;
                self.this()
            }
            "splice" => self.do_splice(&args),
            "toReversed" => {
                let mut items = self.items();
                items.reverse();
                Ok(Self::create(items))
            }
            "toSorted" => Ok(Self::create(sort_items(self.items(), &args)?)),
            "toSpliced" => {
                let mut items = self.items();
                splice_items(&mut items, &args)?;
                Ok(Self::create(items))
            }
            "toString" => Ok(self.join_with(",")?.into()),
            "unshift" => self.do_unshift(args),
            "with" => self.do_with(&args),
            _ => return Err(JSError::MethodNotFound(name.into())),
        }?;
        engine.set_register(Register::AX, result);
        Ok(())
    }

    /// Builds an array literal: `args[0]` is the element count, and the
    /// elements were pushed onto the stack in order.
    fn do_new(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        args: &[Operand],
    ) -> Result<(), JSError> {
        let count = match args.len() {
            0 => 0,
            1 => engine.resolve_value(&args[0], scope)?.to_number()? as usize,
            _ => {
                return Err(JSError::UnknownError(format!(
                    "Too many arguments {}",
                    args.len()
                )));
            }
        };
        let mut ags = vec![];
        for _ in 0..count {
            let val = engine
                .stack
                .pop()
                .ok_or_else(|| JSError::UnknownError("Array element missing from stack".into()))?;
            ags.push(val);
        }
        // Elements were pushed first-to-last, so they pop last-to-first.
        ags.reverse();
        engine.set_register(Register::AX, Self::create(ags));
        Ok(())
    }

    fn do_is_array(args: &[JsValue]) -> Result<JsValue, JSError> {
        Ok(as_array(&arg(args, 0)).is_some().into())
    }

    /// `Array.from(arrayOrString)`: a copy of an array, or a string's
    /// characters.
    fn do_from(args: &[JsValue]) -> Result<JsValue, JSError> {
        if !matches!(&arg(args, 1), JsValue::Undefined) {
            return Err(JSError::NotSupported(
                "Array.from with a map function needs a callback, and the VM can't call JS functions yet"
                    .into(),
            ));
        }
        let source = arg(args, 0);
        if let Some(array) = as_array(&source) {
            return Ok(Self::create(array.items()));
        }
        match &source {
            JsValue::Str(s) => Ok(Self::create(
                s.chars().map(|c| JsValue::from(c.to_string())).collect(),
            )),
            JsValue::Null | JsValue::Undefined => Err(JSError::TypeError(format!(
                "{} is not iterable",
                source.to_js_string()?
            ))),
            // Non-iterable values have no elements.
            _ => Ok(Self::create(vec![])),
        }
    }

    fn do_at(&self, args: &[JsValue]) -> Result<JsValue, JSError> {
        let len = self.len() as f64;
        let n = to_integer(&arg(args, 0))?;
        let k = if n < 0.0 { len + n } else { n };
        Ok(if k < 0.0 || k >= len {
            JsValue::Undefined
        } else {
            self.items.borrow()[k as usize].clone()
        })
    }

    fn do_concat(&self, args: &[JsValue]) -> Result<JsValue, JSError> {
        let mut items = self.items();
        for a in args {
            match as_array(a) {
                Some(array) => items.extend(array.items()),
                None => items.push(a.clone()),
            }
        }
        Ok(Self::create(items))
    }

    /// `copyWithin(target, start, end)`: copies `start..end` over the
    /// elements from `target`, without changing the length.
    fn do_copy_within(&self, args: &[JsValue]) -> Result<JsValue, JSError> {
        let len = self.len();
        let target = relative_index(&arg(args, 0), len, 0)?;
        let start = relative_index(&arg(args, 1), len, 0)?;
        let end = relative_index(&arg(args, 2), len, len)?;
        if start < end {
            let copied = self.items.borrow()[start..end].to_vec();
            let mut items = self.items.borrow_mut();
            for (offset, value) in copied.into_iter().enumerate() {
                match items.get_mut(target + offset) {
                    Some(slot) => *slot = value,
                    None => break,
                }
            }
        }
        self.this()
    }

    fn do_fill(&self, args: &[JsValue]) -> Result<JsValue, JSError> {
        let len = self.len();
        let value = arg(args, 0);
        let start = relative_index(&arg(args, 1), len, 0)?;
        let end = relative_index(&arg(args, 2), len, len)?;
        if start < end {
            self.items.borrow_mut()[start..end].fill(value);
        }
        self.this()
    }

    /// `flat(depth = 1)`: nested arrays up to `depth` levels are spliced in.
    fn do_flat(&self, args: &[JsValue]) -> Result<JsValue, JSError> {
        let depth = match &arg(args, 0) {
            JsValue::Undefined => 1.0,
            _ => to_integer(&arg(args, 0))?,
        };
        let mut out = vec![];
        flatten_into(&self.items(), depth, &mut out);
        Ok(Self::create(out))
    }

    /// `includes(value, from)`: SameValueZero, so `NaN` finds `NaN`.
    fn do_includes(&self, args: &[JsValue]) -> Result<JsValue, JSError> {
        let target = arg(args, 0);
        let from = relative_index(&arg(args, 1), self.len(), 0)?;
        Ok(self.items.borrow()[from..].contains(&target).into())
    }

    /// `indexOf(value, from)`: strict equality, so `NaN` is never found.
    fn do_index_of(&self, args: &[JsValue]) -> Result<JsValue, JSError> {
        let target = arg(args, 0);
        let from = relative_index(&arg(args, 1), self.len(), 0)?;
        let found = self.items.borrow()[from..]
            .iter()
            .position(|v| v.strict_equals(&target))
            .map(|i| i + from);
        Ok(index_result(found))
    }

    /// `lastIndexOf(value, from)`: searches backwards from `from`, which
    /// defaults to the last element.
    fn do_last_index_of(&self, args: &[JsValue]) -> Result<JsValue, JSError> {
        let target = arg(args, 0);
        let len = self.len() as f64;
        let last = if args.len() < 2 {
            len - 1.0
        } else {
            let n = to_integer(&args[1])?;
            if n < 0.0 { len + n } else { n.min(len - 1.0) }
        };
        if last < 0.0 {
            return Ok(index_result(None));
        }
        let found = self.items.borrow()[..=last as usize]
            .iter()
            .rposition(|v| v.strict_equals(&target));
        Ok(index_result(found))
    }

    fn do_join(&self, args: &[JsValue]) -> Result<JsValue, JSError> {
        let separator = match &arg(args, 0) {
            JsValue::Undefined => ",".to_string(),
            sep => sep.to_js_string()?,
        };
        Ok(self.join_with(&separator)?.into())
    }

    /// Joins the elements' string forms. `null` and `undefined` become "",
    /// nested arrays are joined with ",", and a cycle joins as "".
    pub fn join_with(&self, separator: &str) -> Result<String, JSError> {
        let me = self as *const Array;
        if JOINING.with(|j| j.borrow().contains(&me)) {
            return Ok(String::new());
        }
        JOINING.with(|j| j.borrow_mut().push(me));
        let result = self
            .items()
            .iter()
            .map(|v| match v {
                JsValue::Null | JsValue::Undefined => Ok(String::new()),
                _ => to_string_value(v),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|parts| parts.join(separator));
        JOINING.with(|j| j.borrow_mut().pop());
        result
    }

    fn do_push(&self, args: Vec<JsValue>) -> Result<JsValue, JSError> {
        let mut items = self.items.borrow_mut();
        items.extend(args);
        Ok((items.len() as f64).into())
    }

    fn do_shift(&self) -> Result<JsValue, JSError> {
        let mut items = self.items.borrow_mut();
        Ok(if items.is_empty() {
            JsValue::Undefined
        } else {
            items.remove(0)
        })
    }

    fn do_slice(&self, args: &[JsValue]) -> Result<JsValue, JSError> {
        let len = self.len();
        let start = relative_index(&arg(args, 0), len, 0)?;
        let end = relative_index(&arg(args, 1), len, len)?;
        let items = if start < end {
            self.items.borrow()[start..end].to_vec()
        } else {
            vec![]
        };
        Ok(Self::create(items))
    }

    /// `splice(start, deleteCount, ...items)`: returns the removed elements.
    fn do_splice(&self, args: &[JsValue]) -> Result<JsValue, JSError> {
        let removed = splice_items(&mut self.items.borrow_mut(), args)?;
        Ok(Self::create(removed))
    }

    fn do_unshift(&self, args: Vec<JsValue>) -> Result<JsValue, JSError> {
        let mut items = self.items.borrow_mut();
        items.splice(0..0, args);
        Ok((items.len() as f64).into())
    }

    /// `with(index, value)`: a copy with one element replaced.
    fn do_with(&self, args: &[JsValue]) -> Result<JsValue, JSError> {
        let len = self.len() as f64;
        let n = to_integer(&arg(args, 0))?;
        let k = if n < 0.0 { len + n } else { n };
        if k < 0.0 || k >= len {
            return Err(JSError::RangeError(format!("Invalid index : {n}")));
        }
        let mut items = self.items();
        items[k as usize] = arg(args, 1);
        Ok(Self::create(items))
    }

    /// Element `i`, or `undefined` past the end.
    pub fn get_index(&self, i: usize) -> JsValue {
        self.items
            .borrow()
            .get(i)
            .cloned()
            .unwrap_or(JsValue::Undefined)
    }

    /// Sets element `i`, padding with `undefined` when `i` is past the end.
    pub fn set_index(&self, i: usize, value: JsValue) {
        let mut items = self.items.borrow_mut();
        if i >= items.len() {
            items.resize(i + 1, JsValue::Undefined);
        }
        items[i] = value;
    }

    /// Sets `length`: truncates, or pads with `undefined`.
    fn set_length(&self, value: &JsValue) -> Result<(), JSError> {
        let n = value.to_number()?;
        if n < 0.0 || n.fract() != 0.0 || n > u32::MAX as f64 {
            return Err(JSError::RangeError("Invalid array length".into()));
        }
        self.items
            .borrow_mut()
            .resize(n as usize, JsValue::Undefined);
        Ok(())
    }
}

/// The `i`th argument, or `undefined` if it wasn't passed.
fn arg(args: &[JsValue], i: usize) -> JsValue {
    args.get(i).cloned().unwrap_or(JsValue::Undefined)
}

/// The array inside a value, if it is one.
fn as_array(value: &JsValue) -> Option<&Array> {
    match value {
        JsValue::Object(object) => object.as_array(),
        _ => None,
    }
}

/// ToIntegerOrInfinity: truncates toward zero; `NaN` is 0.
fn to_integer(value: &JsValue) -> Result<f64, JSError> {
    let n = value.to_number()?;
    Ok(if n.is_nan() { 0.0 } else { n.trunc() })
}

/// A position argument where negative values count back from the end,
/// clamped to `0..=len`. `undefined` means `default`.
fn relative_index(value: &JsValue, len: usize, default: usize) -> Result<usize, JSError> {
    if matches!(value, JsValue::Undefined) {
        return Ok(default);
    }
    let n = to_integer(value)?;
    let len = len as f64;
    Ok(if n < 0.0 {
        (len + n).max(0.0)
    } else {
        n.min(len)
    } as usize)
}

fn index_result(index: Option<usize>) -> JsValue {
    index.map_or(-1.0, |i| i as f64).into()
}

/// ToString, except that arrays join their elements (`[1, [2, 3]]` is
/// `"1,2,3"`) rather than becoming `"[object Object]"`.
fn to_string_value(value: &JsValue) -> Result<String, JSError> {
    match as_array(value) {
        Some(array) => array.join_with(","),
        None => value.to_js_string(),
    }
}

fn flatten_into(items: &[JsValue], depth: f64, out: &mut Vec<JsValue>) {
    for item in items {
        match as_array(item) {
            Some(nested) if depth >= 1.0 => flatten_into(&nested.items(), depth - 1.0, out),
            _ => out.push(item.clone()),
        }
    }
}

/// The default sort: by string form, compared as UTF-16 code units, with
/// `undefined` last. Stable, like JS's. Comparator functions need callbacks.
fn sort_items(items: Vec<JsValue>, args: &[JsValue]) -> Result<Vec<JsValue>, JSError> {
    match &arg(args, 0) {
        JsValue::Undefined => {}
        JsValue::Function(_) => {
            return Err(JSError::NotSupported(
                "sorting with a comparator needs a callback, and the VM can't call JS functions yet"
                    .into(),
            ));
        }
        _ => {
            return Err(JSError::TypeError(
                "The comparison function must be either a function or undefined".into(),
            ));
        }
    }
    let (undefined, defined): (Vec<_>, Vec<_>) = items
        .into_iter()
        .partition(|v| matches!(v, JsValue::Undefined));
    let mut keyed = defined
        .into_iter()
        .map(|v| Ok((to_string_value(&v)?.encode_utf16().collect::<Vec<_>>(), v)))
        .collect::<Result<Vec<_>, JSError>>()?;
    keyed.sort_by(|(a, _), (b, _)| a.cmp(b));
    Ok(keyed.into_iter().map(|(_, v)| v).chain(undefined).collect())
}

/// Splices `items` in place per `splice(start, deleteCount, ...insert)`,
/// returning the removed elements.
fn splice_items(items: &mut Vec<JsValue>, args: &[JsValue]) -> Result<Vec<JsValue>, JSError> {
    let len = items.len();
    let start = relative_index(&arg(args, 0), len, 0)?;
    let delete = match args.len() {
        0 => 0,
        1 => len - start,
        _ => to_integer(&args[1])?.clamp(0.0, (len - start) as f64) as usize,
    };
    let insert = args.iter().skip(2).cloned();
    Ok(items.splice(start..start + delete, insert).collect())
}

/// An array index property name: a canonical non-negative integer, like
/// `"0"` or `"12"` (not `"01"` or `"1.0"`).
fn array_index(prop: &str) -> Option<usize> {
    let i = prop.parse::<u32>().ok()?;
    (i != u32::MAX && i.to_string() == prop).then_some(i as usize)
}

impl Object for Array {
    fn prototype(&self) -> RcPrototype {
        PROPS.with(|p| p.clone())
    }

    fn as_array(&self) -> Option<&Array> {
        Some(self)
    }

    fn get_props(&self) -> Vec<String> {
        (0..self.len())
            .map(|i| i.to_string())
            .chain(["length".to_string()])
            .collect()
    }

    fn get_methods(&self) -> Vec<String> {
        METHODS
            .iter()
            .chain(CALLBACK_METHODS)
            .chain(ITERATOR_METHODS)
            .map(|m| m.to_string())
            .collect()
    }

    fn get_prop(&self, prop: &str) -> Result<JsValue, JSError> {
        if prop == "length" {
            return Ok((self.len() as f64).into());
        }
        if let Some(i) = array_index(prop) {
            return Ok(self
                .items
                .borrow()
                .get(i)
                .cloned()
                .unwrap_or(JsValue::Undefined));
        }
        self.prototype().0.borrow().get(prop)
    }

    fn set_prop(&self, prop: &str, value: JsValue) -> Result<(), JSError> {
        if prop == "length" {
            return self.set_length(&value);
        }
        if let Some(i) = array_index(prop) {
            self.set_index(i, value);
            return Ok(());
        }
        Err(JSError::NotSupported(format!(
            "setting the non-index property '{prop}' on an array"
        )))
    }

    fn has_prop(&self, prop: &str) -> bool {
        prop == "length"
            || array_index(prop).is_some_and(|i| i < self.len())
            || self.prototype().0.borrow().get(prop).is_ok()
    }

    fn has_method(&self, method: &str) -> bool {
        self.prototype()
            .0
            .borrow()
            .get(method)
            .is_ok_and(|m| matches!(&m, JsValue::Function(_)))
    }

    fn call(
        &self,
        engine: &mut JSEngine,
        scope: &Rc<Scope>,
        method: &Func,
        args: &[Operand],
    ) -> Result<(), JSError> {
        if let JsValue::Function(func) = &self.get_prop(&method.name)? {
            self.call_func(func, engine, scope, args)
        } else {
            Err(JSError::RefereneError(method.to_string()))
        }
    }

    fn type_of(&self) -> String {
        "Array".into()
    }
}

#[cfg(test)]
mod tests {
    use super::Array;
    use crate::{error::JSError, executor::engine::JSEngine, primitive::JsValue};

    /// Run `src` and return variable `s` as a string.
    fn eval(src: &str) -> String {
        let mut engine = JSEngine::new();
        engine.exec(src).unwrap_or_else(|e| panic!("{src}: {e}"));
        engine.scope.get_variable("s").unwrap().to_string()
    }

    fn exec_err(src: &str) -> JSError {
        let mut engine = JSEngine::new();
        engine.exec(src).expect_err(src)
    }

    #[test]
    fn test_literal_and_to_string() {
        assert_eq!(eval("let s = [1, 2, 3].toString()"), "1,2,3");
        assert_eq!(
            eval("let x = 1; let a = [x, 1 + 1, 2 * 2, 'z']; x = 9; let s = a.toString()"),
            "1,2,4,z"
        );
        assert_eq!(eval("let s = [].toString()"), "");
        assert_eq!(eval("let s = [1, [2, [3, 4]], 5].toString()"), "1,2,3,4,5");
    }

    #[test]
    fn test_length() {
        assert_eq!(eval("let s = [1, 2, 3].length"), "3");
        assert_eq!(eval("let s = [].length"), "0");
        assert_eq!(eval("let a = [1]; a.push(2, 3); let s = a.length"), "3");
    }

    #[test]
    fn test_join() {
        assert_eq!(eval("let s = [1, 2, 3].join()"), "1,2,3");
        assert_eq!(eval("let s = [1, 2, 3].join(' - ')"), "1 - 2 - 3");
        assert_eq!(eval("let s = [1, 2, 3].join('')"), "123");
        assert_eq!(
            eval("let s = ['a', null, 'b'].join('|')"),
            "a||b",
            "null joins as empty"
        );
        assert_eq!(eval("let s = [true, 1.5, 0 / 0].join()"), "true,1.5,NaN");
        assert_eq!(
            eval("let a = [1, 2]; a.push(a); let s = a.join('-')"),
            "1-2-",
            "a cycle joins as empty"
        );
    }

    #[test]
    fn test_push_pop_shift_unshift() {
        assert_eq!(
            eval("let a = [1]; let s = a.push(2, 3)"),
            "3",
            "push returns the new length"
        );
        assert_eq!(eval("let a = [1]; a.push(2, 3); let s = a.join()"), "1,2,3");
        assert_eq!(eval("let a = [1, 2, 3]; let s = a.pop()"), "3");
        assert_eq!(eval("let a = [1, 2, 3]; a.pop(); let s = a.join()"), "1,2");
        assert_eq!(eval("let a = []; let s = a.pop()"), "undefined");
        assert_eq!(eval("let a = [1, 2, 3]; let s = a.shift()"), "1");
        assert_eq!(
            eval("let a = [1, 2, 3]; a.shift(); let s = a.join()"),
            "2,3"
        );
        assert_eq!(eval("let a = []; let s = a.shift()"), "undefined");
        assert_eq!(eval("let a = [3]; let s = a.unshift(1, 2)"), "3");
        assert_eq!(
            eval("let a = [3]; a.unshift(1, 2); let s = a.join()"),
            "1,2,3"
        );
    }

    #[test]
    fn test_mutators_return_the_same_array() {
        for method in ["reverse()", "sort()", "fill(0)", "copyWithin(0, 1)"] {
            let src = format!("let a = [3, 1, 2]; let s = a.{method} === a");
            assert_eq!(eval(&src), "true", "{method}");
        }
    }

    #[test]
    fn test_reverse_and_to_reversed() {
        assert_eq!(
            eval("let a = [1, 2, 3]; a.reverse(); let s = a.join()"),
            "3,2,1"
        );
        assert_eq!(
            eval("let a = [1, 2, 3]; let s = a.toReversed().join() + ' ' + a.join()"),
            "3,2,1 1,2,3"
        );
    }

    #[test]
    fn test_sort() {
        assert_eq!(
            eval("let a = [3, 1, 2]; a.sort(); let s = a.join()"),
            "1,2,3"
        );
        assert_eq!(
            eval("let a = [10, 9, 1, 100]; a.sort(); let s = a.join()"),
            "1,10,100,9",
            "sorted as strings"
        );
        assert_eq!(
            eval("let a = ['b', 'B', 'a', 'A']; a.sort(); let s = a.join()"),
            "A,B,a,b"
        );
        assert_eq!(
            eval("let a = [3, null, 1]; a.sort(); let s = a.join('|')"),
            "1|3|",
            "null sorts as 'null'"
        );
        assert_eq!(
            eval("let a = [3, 1, 2]; let s = a.toSorted().join() + ' ' + a.join()"),
            "1,2,3 3,1,2"
        );
        assert!(matches!(exec_err("[1].sort(1)"), JSError::TypeError(_)));
    }

    #[test]
    fn test_slice() {
        assert_eq!(eval("let s = [1, 2, 3, 4].slice(1).join()"), "2,3,4");
        assert_eq!(eval("let s = [1, 2, 3, 4].slice(1, 3).join()"), "2,3");
        assert_eq!(eval("let s = [1, 2, 3, 4].slice(0 - 2).join()"), "3,4");
        assert_eq!(eval("let s = [1, 2, 3, 4].slice(1, 0 - 1).join()"), "2,3");
        assert_eq!(eval("let s = [1, 2, 3, 4].slice(3, 1).join()"), "");
        assert_eq!(eval("let s = [1, 2, 3, 4].slice(10).join()"), "");
        assert_eq!(
            eval("let a = [1, 2]; let s = a.slice() === a"),
            "false",
            "a copy"
        );
    }

    #[test]
    fn test_splice_and_to_spliced() {
        assert_eq!(
            eval("let a = [1, 2, 3, 4]; let s = a.splice(1, 2).join()"),
            "2,3",
            "returns the removed elements"
        );
        assert_eq!(
            eval("let a = [1, 2, 3, 4]; a.splice(1, 2); let s = a.join()"),
            "1,4"
        );
        assert_eq!(
            eval("let a = [1, 2, 3, 4]; a.splice(1, 1, 'x', 'y'); let s = a.join()"),
            "1,x,y,3,4"
        );
        assert_eq!(
            eval("let a = [1, 2, 3, 4]; a.splice(2); let s = a.join()"),
            "1,2",
            "no count removes the rest"
        );
        assert_eq!(
            eval("let a = [1, 2, 3]; a.splice(0 - 1, 1); let s = a.join()"),
            "1,2"
        );
        assert_eq!(
            eval("let a = [1, 2, 3]; a.splice(1, 0, 9); let s = a.join()"),
            "1,9,2,3"
        );
        assert_eq!(eval("let a = [1, 2, 3]; let s = a.splice().length"), "0");
        assert_eq!(
            eval("let a = [1, 2, 3]; let s = a.toSpliced(1, 1, 'x').join() + ' ' + a.join()"),
            "1,x,3 1,2,3"
        );
    }

    #[test]
    fn test_fill_and_copy_within() {
        assert_eq!(
            eval("let a = [1, 2, 3]; a.fill(0); let s = a.join()"),
            "0,0,0"
        );
        assert_eq!(
            eval("let a = [1, 2, 3, 4]; a.fill(0, 1, 3); let s = a.join()"),
            "1,0,0,4"
        );
        assert_eq!(
            eval("let a = [1, 2, 3, 4]; a.fill(0, 0 - 1); let s = a.join()"),
            "1,2,3,0"
        );
        assert_eq!(
            eval("let a = [1, 2, 3, 4, 5]; a.copyWithin(0, 3); let s = a.join()"),
            "4,5,3,4,5"
        );
        assert_eq!(
            eval("let a = [1, 2, 3, 4, 5]; a.copyWithin(1, 0, 2); let s = a.join()"),
            "1,1,2,4,5"
        );
        assert_eq!(
            eval("let a = [1, 2, 3]; a.copyWithin(2, 0); let s = a.join()"),
            "1,2,1",
            "length is unchanged"
        );
    }

    #[test]
    fn test_concat() {
        assert_eq!(
            eval("let s = [1].concat([2, 3], 4, [[5]]).join('|')"),
            "1|2|3|4|5"
        );
        assert_eq!(
            eval("let a = [1]; let b = a.concat(); let s = (b === a) + ' ' + b.join()"),
            "false 1"
        );
    }

    #[test]
    fn test_flat() {
        assert_eq!(eval("let s = [1, [2, [3, [4]]]].flat().length"), "3");
        assert_eq!(eval("let s = [1, [2, [3, [4]]]].flat(2).length"), "4");
        assert_eq!(
            eval("let s = [1, [2, [3, [4]]]].flat(1 / 0).length"),
            "4",
            "Infinity flattens everything"
        );
        assert_eq!(eval("let s = [1, [2]].flat(0).length"), "2");
    }

    #[test]
    fn test_index_of_last_index_of_includes() {
        assert_eq!(eval("let s = [1, 2, 3, 2].indexOf(2)"), "1");
        assert_eq!(eval("let s = [1, 2, 3, 2].indexOf(2, 2)"), "3");
        assert_eq!(eval("let s = [1, 2, 3].indexOf(4)"), "-1");
        assert_eq!(
            eval("let s = [1, 2, 3].indexOf('2')"),
            "-1",
            "strict equality"
        );
        assert_eq!(eval("let s = [1, 2, 3, 2].lastIndexOf(2)"), "3");
        assert_eq!(eval("let s = [1, 2, 3, 2].lastIndexOf(2, 2)"), "1");
        assert_eq!(eval("let s = [1, 2, 3, 2].lastIndexOf(2, 0 - 2)"), "1");
        assert_eq!(eval("let s = [].lastIndexOf(1)"), "-1");
        assert_eq!(eval("let s = [1, 2, 3].includes(2)"), "true");
        assert_eq!(eval("let s = [1, 2, 3].includes(2, 2)"), "false");
        assert_eq!(
            eval("let s = [0 / 0].includes(0 / 0)"),
            "true",
            "includes finds NaN"
        );
        assert_eq!(
            eval("let s = [0 / 0].indexOf(0 / 0)"),
            "-1",
            "indexOf never finds NaN"
        );
        assert_eq!(
            eval("let a = [1]; let b = [a]; let s = b.indexOf(a)"),
            "0",
            "objects by identity"
        );
        assert_eq!(eval("let b = [[1]]; let s = b.indexOf([1])"), "-1");
    }

    #[test]
    fn test_at_and_with() {
        assert_eq!(eval("let s = [1, 2, 3].at(0)"), "1");
        assert_eq!(eval("let s = [1, 2, 3].at(0 - 1)"), "3");
        assert_eq!(eval("let s = [1, 2, 3].at(5)"), "undefined");
        assert_eq!(
            eval("let a = [1, 2, 3]; let s = a.with(1, 'x').join() + ' ' + a.join()"),
            "1,x,3 1,2,3"
        );
        assert_eq!(eval("let s = [1, 2, 3].with(0 - 1, 'x').join()"), "1,2,x");
        assert!(matches!(exec_err("[1].with(5, 0)"), JSError::RangeError(_)));
    }

    #[test]
    fn test_static_methods() {
        assert_eq!(eval("let s = Array.isArray([1])"), "true");
        assert_eq!(eval("let s = Array.isArray('abc')"), "false");
        assert_eq!(eval("let s = Array.of(1, 'a', true).join()"), "1,a,true");
        assert_eq!(eval("let s = Array.from('abc').join('-')"), "a-b-c");
        assert_eq!(
            eval("let a = [1, 2]; let b = Array.from(a); let s = (b === a) + ' ' + b.join()"),
            "false 1,2"
        );
        assert_eq!(eval("let s = Array.from(5).length"), "0");
        assert!(matches!(
            exec_err("Array.from(null)"),
            JSError::TypeError(_)
        ));
    }

    #[test]
    fn test_callback_and_iterator_methods_are_reported() {
        for method in [
            "map", "filter", "forEach", "reduce", "find", "some", "every", "flatMap",
        ] {
            let err = exec_err(&format!("[1].{method}(1)"));
            assert!(matches!(err, JSError::NotSupported(_)), "{method}: {err}");
        }
        for method in ["keys", "values", "entries"] {
            assert!(
                matches!(
                    exec_err(&format!("[1].{method}()")),
                    JSError::NotSupported(_)
                ),
                "{method}"
            );
        }
    }

    #[test]
    fn test_index_and_length_properties() {
        let array = Array::create(vec![1.0.into(), 2.0.into(), 3.0.into()]);
        let JsValue::Object(object) = &array else {
            unreachable!()
        };
        assert_eq!(object.get_prop("1").unwrap(), 2.0.into());
        assert_eq!(object.get_prop("7").unwrap(), JsValue::Undefined);
        assert!(object.has_prop("2") && !object.has_prop("3") && object.has_prop("push"));
        object.set_prop("4", "x".into()).unwrap();
        assert_eq!(object.get_prop("length").unwrap(), 5.0.into());
        assert_eq!(
            object.get_prop("3").unwrap(),
            JsValue::Undefined,
            "the gap is filled"
        );
        object.set_prop("length", 2.0.into()).unwrap();
        assert_eq!(object.as_array().unwrap().join_with(",").unwrap(), "1,2");
        assert!(matches!(
            object.set_prop("length", (-1.0).into()),
            Err(JSError::RangeError(_))
        ));
        assert!(matches!(
            object.set_prop("length", 1.5.into()),
            Err(JSError::RangeError(_))
        ));
    }
}
