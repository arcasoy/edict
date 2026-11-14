pub struct Widget;

pub fn run() -> u32 {
    Widget.process()
}

#[cfg(test)]
mod tests {
    #[test]
    fn uses_c_in_tests_only() {
        assert_eq!(super::Widget.process(), 2);
    }
}
