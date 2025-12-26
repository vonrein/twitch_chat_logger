use std::process::{Command, Stdio};
use std::thread;
use crate::globals::IS_SERVER_MODE;
use std::sync::atomic::{AtomicBool, Ordering};

// Global toggle for sound
pub static IS_SOUND_DISABLED: AtomicBool = AtomicBool::new(false);

pub fn play_sound() {
    // 1. Check server mode
    if *IS_SERVER_MODE.lock().unwrap() {
        return;
    }

    // 2. Check global mute
    if IS_SOUND_DISABLED.load(Ordering::Relaxed) {
        return;
    }

    // 3. Fire and forget (Spawn thread to avoid blocking main loop)
    thread::spawn(|| {
        if let Some(mut path) = home::home_dir() {
            path.push(".rustTwitchLogger/tick.ogg");

            if path.exists() {
                // "paplay" is the standard CLI for PulseAudio/PipeWire (Arch default)
                // "aplay" is the fallback for pure ALSA
                let _ = Command::new("paplay")
                .arg(path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            }
        }
    });
}
