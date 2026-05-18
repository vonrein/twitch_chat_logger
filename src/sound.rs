use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};
use once_cell::sync::Lazy;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::globals::IS_SERVER_MODE;

// Global toggle for sound
pub static IS_SOUND_DISABLED: AtomicBool = AtomicBool::new(false);

// Lazy initialization: The thread is ONLY spawned the first time this is used.
pub static SOUND_TRIGGER: Lazy<Sender<()>> = Lazy::new(start_sound_thread);

pub fn play_sound() {
    // 1. Check server mode
    // If we are in server mode, we return here.
    // SOUND_TRIGGER is never touched -> Thread never spawns.
    if *IS_SERVER_MODE.lock().unwrap() {
        return;
    }

    // 2. Check global mute (includes --quiet)
    // If muted, we return here.
    // SOUND_TRIGGER is never touched -> Thread never spawns.
    if IS_SOUND_DISABLED.load(Ordering::Relaxed) {
        return;
    }

    // 3. Only now do we access the Lazy static.
    // This triggers start_sound_thread() ONLY if we actually intend to make a sound.
    let _ = SOUND_TRIGGER.send(());
}

fn start_sound_thread() -> Sender<()> {
    let (tx, rx) = mpsc::channel::<()>();

    // Safety Guard:
    // Even if logic slips up and calls this in server mode,
    // we refuse to spawn the OS thread.
    if *IS_SERVER_MODE.lock().unwrap() {
        return tx;
    }

    thread::spawn(move || {
        let mut path_buf = home::home_dir().unwrap_or_default();
        path_buf.push(".rustTwitchLogger/tick.ogg");

        let path_exists = path_buf.exists();
        let cooldown = Duration::from_millis(200);
        let mut last_played = Instant::now() - cooldown;

        while let Ok(()) = rx.recv() {
            if last_played.elapsed() >= cooldown {
                if path_exists {
                    // Synchronous playback prevents overlapping sounds
                    let _ = Command::new("paplay")
                    .arg(&path_buf)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();

                    last_played = Instant::now();
                }
            }
        }
    });

    tx
}
