pub mod runtime;

use std::cell::RefCell;
use std::sync::atomic::AtomicU64;
use std::sync::Mutex;

pub struct Db {
    pub path: String,
}

static mut COUNTER: u64 = 0;
static CACHE: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static HITS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static LOCAL: RefCell<u32> = RefCell::new(0);
}

pub fn touch() {
    let _ = &CACHE;
    let _ = &HITS;
}
