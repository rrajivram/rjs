use std::sync::atomic::AtomicU64;

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd)]
pub struct Symbol {
    name: Option<String>,
    value: u64,
}

static SYMBOL_COUNTER: AtomicU64 = AtomicU64::new(0);
impl Symbol {
    fn new(name: Option<String>) -> Self {
        Self {
            name,
            value: SYMBOL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }
}
