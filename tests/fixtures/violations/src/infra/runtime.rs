use std::sync::Mutex;

pub static RUNTIME: Mutex<u8> = Mutex::new(0);
