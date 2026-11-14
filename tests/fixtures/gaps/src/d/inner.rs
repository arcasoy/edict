pub struct Thing;

pub fn inner() -> u32 {
    let _ = crate::c::entry as fn() -> u32;
    1
}

pub fn shadow() {}
