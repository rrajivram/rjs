use std::collections::BTreeMap;

use crate::primitive::ArcPrimitive;

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Object {
    props: BTreeMap<String, ArcPrimitive>,
}

impl Object {
    pub fn new(props: BTreeMap<String, ArcPrimitive>) -> Self {
        Self { props }
    }

    /// Whether `key` is one of this object's own properties.
    pub fn has_property(&self, key: &str) -> bool {
        self.props.contains_key(key)
    }
}
