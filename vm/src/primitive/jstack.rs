use std::collections::VecDeque;

use crate::primitive::RcPrimitive;

#[derive(Debug, Clone)]
pub enum StackValue {
    Number(isize),
    Value(RcPrimitive),
}

#[derive(Debug, Clone, Default)]
pub struct JSStack {
    stack: VecDeque<StackValue>,
}

impl JSStack {
    pub fn push(&mut self, value: StackValue) {
        self.stack.push_back(value);
    }

    pub fn pop(&mut self) -> Option<StackValue> {
        self.stack.pop_back()
    }

    pub fn peek(&mut self) -> Option<&StackValue> {
        self.stack.back()
    }
}

impl From<isize> for StackValue {
    fn from(value: isize) -> Self {
        StackValue::Number(value)
    }
}

impl From<RcPrimitive> for StackValue {
    fn from(value: RcPrimitive) -> Self {
        Self::Value(value)
    }
}
