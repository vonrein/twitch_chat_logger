use rodio::{OutputStream, Sink, Source};
use std::fs;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use once_cell::sync::Lazy;
use home;

// Step 1: Create an enum to represent the waveform choice.
#[derive(Debug, Clone, Copy)] // Add Clone and Copy
pub enum WaveformType {
    Square,
    Sine,
    Sawtooth,
    Triangle,
}

// Step 2: Update the controller to hold the waveform state.
pub struct SoundController {
    pub tx: Sender<()>,
    pub frequency: Arc<Mutex<f32>>,
    pub waveform: Arc<Mutex<WaveformType>>, // New field
}

pub static SOUND_CONTROLLER: Lazy<SoundController> = Lazy::new(start_sound_thread);

pub fn play_sound() {
    if let Err(e) = SOUND_CONTROLLER.tx.send(()) {
        eprintln!("Failed to send sound trigger: {}", e);
    }
}

fn get_initial_frequency() -> f32 {
    const DEFAULT_FREQ: f32 = 69.0;

    // 1. Try to get the user's home directory.
    if let Some(mut path) = home::home_dir() {
        // 2. Append your configuration path to it.
        path.push(".rustTwitchLogger/frequency.txt");

        // 3. The rest of the logic is the same, just using the new path.
        match fs::read_to_string(&path) {
            Ok(content) => match content.trim().parse::<f32>() {
                Ok(freq) => {
                    println!("Loaded initial frequency from '{}': {} Hz", path.display(), freq);
                    freq
                }
                Err(_) => {
                    eprintln!("'{}' contains invalid data. Using default frequency.", path.display());
                    DEFAULT_FREQ
                }
            },
            Err(_) => {
                // This will now trigger if the file is not in the config directory.
                println!("'{}' not found. Using default frequency.", path.display());
                DEFAULT_FREQ
            }
        }
    } else {
        // Fallback if the home directory cannot be determined.
        eprintln!("Could not find home directory. Using default frequency.");
        DEFAULT_FREQ
    }
}


fn start_sound_thread() -> SoundController {
    let (tx, rx) = mpsc::channel::<()>();

    let initial_freq = get_initial_frequency();
    let frequency_arc = Arc::new(Mutex::new(initial_freq));
    // Step 3: Create shared state for the waveform, defaulting to Square.
    let waveform_arc = Arc::new(Mutex::new(WaveformType::Square));

    let frequency_for_thread = Arc::clone(&frequency_arc);
    let waveform_for_thread = Arc::clone(&waveform_arc);

    thread::spawn(move || {
        let (_stream, stream_handle) =
        OutputStream::try_default().expect("Failed to get audio output stream");

        while let Ok(()) = rx.recv() {
            if let Ok(sink) = Sink::try_new(&stream_handle) {
                // Step 4: Get current settings from shared state.
                let current_freq = *frequency_for_thread.lock().unwrap();
                let current_waveform = *waveform_for_thread.lock().unwrap();

                // Use the new WaveformGenerator.
                let source = WaveformGenerator::new(
                    current_waveform,
                    current_freq,
                    Duration::from_millis(150),
                );
                sink.append(source);
                sink.detach();
            }
        }
    });

    // Step 5: Return the complete controller.
    SoundController {
        tx,
        frequency: frequency_arc,
        waveform: waveform_arc,
    }
}

// ====== NEW Generic Waveform Generator ======
// This replaces the old SquareWave struct and its impls.
pub struct WaveformGenerator {
    wave_type: WaveformType,
    sample_rate: u32,
    freq: f32,
    duration: Duration,
    elapsed_samples: u32,
}

impl WaveformGenerator {
    pub fn new(wave_type: WaveformType, freq: f32, duration: Duration) -> Self {
        Self {
            wave_type,
            sample_rate: 44100,
            freq,
            duration,
            elapsed_samples: 0,
        }
    }
}

impl Iterator for WaveformGenerator {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let total_samples = (self.duration.as_secs_f32() * self.sample_rate as f32) as u32;
        if self.elapsed_samples >= total_samples {
            return None;
        }

        let period_in_samples = self.sample_rate as f32 / self.freq;

        let value = match self.wave_type {
            WaveformType::Square => {
                let phase = self.elapsed_samples as f32 % period_in_samples;
                if phase < period_in_samples / 2.0 { 0.25 } else { -0.25 }
            }
            WaveformType::Sine => {
                let t = self.elapsed_samples as f32 / self.sample_rate as f32;
                let angular_freq = self.freq * 2.0 * std::f32::consts::PI;
                0.25 * (t * angular_freq).sin()
            }
            WaveformType::Sawtooth => {
                let phase = self.elapsed_samples as f32 % period_in_samples;
                // Normalize phase from [0, period) to [-1, 1) and scale amplitude
                0.25 * ((phase / period_in_samples) * 2.0 - 1.0)
            }
            WaveformType::Triangle => {
                let phase = self.elapsed_samples as f32 % period_in_samples;
                let norm_phase = phase / period_in_samples;
                let val = if norm_phase < 0.5 {
                    4.0 * norm_phase - 1.0
                } else {
                    -4.0 * norm_phase + 3.0
                };
                0.25 * val
            }
        };

        self.elapsed_samples += 1;
        Some(value)
    }
}

impl Source for WaveformGenerator {
    fn current_frame_len(&self) -> Option<usize> { None }
    fn channels(&self) -> u16 { 1 }
    fn sample_rate(&self) -> u32 { self.sample_rate }
    fn total_duration(&self) -> Option<Duration> { Some(self.duration) }
}
