use rodio::{OutputStream, Sink, Source};

use std::sync::mpsc::{self, Sender};

use std::thread;

use std::time::Duration;

use once_cell::sync::Lazy;


pub static SOUND_TX: Lazy<Sender<()>> = Lazy::new(start_sound_thread);


/// Call this function to play the generated sound.

pub fn play_sound() {

    if let Err(e) = SOUND_TX.send(()) {

        eprintln!("Failed to send sound trigger: {}", e);

    }

}


fn start_sound_thread() -> Sender<()> {

    let (tx, rx) = mpsc::channel::<()>();


    thread::spawn(move || {

        let (_stream, stream_handle) = match OutputStream::try_default() {

            Ok(tuple) => tuple,

                  Err(e) => {

                      eprintln!("Failed to get audio output stream: {}", e);

                      return;

                  }

        };


        while let Ok(()) = rx.recv() {

            if let Ok(sink) = Sink::try_new(&stream_handle) {

                let source = SquareWave::new(69.0, Duration::from_millis(150));

                sink.append(source);

                sink.detach();

            }

        }

    });


    tx

}


// ====== SquareWave Generator ======


pub struct SquareWave {

    sample_rate: u32,

    freq: f32,

    duration: Duration,

    elapsed_samples: u32,

}


impl SquareWave {

    pub fn new(freq: f32, duration: Duration) -> Self {

        Self {

            sample_rate: 44100,

            freq,

            duration,

            elapsed_samples: 0,

        }

    }

}


impl Iterator for SquareWave {

    type Item = f32;


    fn next(&mut self) -> Option<Self::Item> {

        let total_samples = (self.duration.as_secs_f32() * self.sample_rate as f32) as u32;

        if self.elapsed_samples >= total_samples {

            return None;

        }


        let t = self.elapsed_samples as f32 / self.sample_rate as f32;

        let value = if (t * self.freq * 2.0 * std::f32::consts::PI).sin() >= 0.0 {

            0.25

        } else {

            -0.25

        };


        self.elapsed_samples += 1;

        Some(value)

    }

}


impl Source for SquareWave {

    fn current_frame_len(&self) -> Option<usize> {

        None

    }


    fn channels(&self) -> u16 {

        1

    }


    fn sample_rate(&self) -> u32 {

        self.sample_rate

    }


    fn total_duration(&self) -> Option<Duration> {

        Some(self.duration)

    }

}
