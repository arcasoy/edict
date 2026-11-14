use crate::billing::Invoice;

pub fn price(i: &Invoice) -> u64 {
    let base = crate::billing::tax::rate(i.0);
    let label = format!("{}", crate::billing::tax::rate(1));
    base + label.len() as u64
}
