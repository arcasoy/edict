// Sibling cycle: stock <-> cart.
pub fn level() -> u32 {
    super::cart::reserve();
    1
}
