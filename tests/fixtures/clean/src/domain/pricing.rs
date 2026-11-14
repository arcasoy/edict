use super::order::Order;

pub fn total(o: &Order) -> u64 {
    o.cents.iter().sum()
}

#[cfg(test)]
mod tests {
    // Test code may do I/O; edict ignores it.
    #[test]
    fn reads() {
        let _ = std::fs::read_to_string("x");
    }
}
