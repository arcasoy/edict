// Sibling cycle: stock <-> cart. Not excused by allow_parent_child.
pub fn level() -> u32 {
    super::cart::reserve();
    1
}
