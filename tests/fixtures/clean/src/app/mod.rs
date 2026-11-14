use crate::domain::{total, Order};
use std::sync::OnceLock;

static CONFIG: OnceLock<String> = OnceLock::new();

pub fn checkout(o: &Order) -> u64 {
    let _ = CONFIG.get();
    let now = std::time::Instant::now();
    let _ = now;
    total(o)
}
