use crate::d::*;

pub fn entry() -> u32 {
    let shadow = 1; // lowercase local, not a call: must not resolve through the glob
    let _t = Thing;
    inner() + shadow
}
