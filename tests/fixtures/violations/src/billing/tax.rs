pub fn rate(cents: u64) -> u64 {
    let _ = super::invoice::Invoice(cents);
    cents / 10
}
