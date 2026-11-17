// Parent <-> child: uses `super::Item` while `shop` calls `cart::sum`.
pub fn sum() -> u32 {
    let _ = super::Item;
    super::stock::level()
}

pub fn reserve() {}
