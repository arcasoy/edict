use super::tax::rate;

pub struct Invoice(pub u64);

pub fn with_tax(i: &Invoice) -> u64 {
    i.0 + rate(i.0)
}
