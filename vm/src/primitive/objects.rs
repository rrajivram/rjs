use std::collections::BTreeMap;

use crate::primitive::ArcPrimitive;

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Object {
    props: BTreeMap<String, ArcPrimitive>,
}
