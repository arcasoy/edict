mod cart;
mod stock;

pub struct Item;

pub fn total() -> u32 {
    cart::sum()
}
