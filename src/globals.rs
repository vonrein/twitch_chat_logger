use once_cell::sync::Lazy;
use std::sync::Mutex;

// This static will be accessible by both main.rs and sound.rs
pub static IS_SERVER_MODE: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));
