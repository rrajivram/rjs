use std::collections::VecDeque;

use crate::{executor::instr::Operand, primitive::JsValue};

#[derive(Debug, Clone)]
pub enum StackValue {
    Number(isize),
    Value(Operand),
}

/// Values pushed for later use (e.g. array elements). Holds resolved values,
/// not references to registers or variables, which may change before the
/// value is popped.
#[derive(Debug, Clone, Default)]
pub struct JSStack {
    stack: VecDeque<JsValue>,
}

impl JSStack {
    pub fn push(&mut self, value: JsValue) {
        self.stack.push_back(value);
    }

    pub fn pop(&mut self) -> Option<JsValue> {
        self.stack.pop_back()
    }

    pub fn peek(&mut self) -> Option<&JsValue> {
        self.stack.back()
    }
}
