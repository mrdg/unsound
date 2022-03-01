extern crate anyhow;

#[macro_use]
extern crate lazy_static;

mod app;
mod engine;
mod env;
mod files;
mod input;
mod pattern;
mod sampler;
mod view;

use anyhow::{anyhow, Result};
use app::Msg;
use assert_no_alloc::*;
use camino::Utf8PathBuf;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use engine::Engine;

#[cfg(debug_assertions)]
#[global_allocator]
static A: AllocDisabler = AllocDisabler;

const SAMPLE_RATE: f64 = 48000.0;
const FRAMES_PER_BUFFER: u32 = 256;

fn main() {
    match run() {
        Ok(_) => {}
        err => {
            eprintln!("error: {:?}", err);
        }
    }
}

fn run() -> Result<()> {
    let (mut app, engine) = app::new()?;
    let stream = run_audio(engine)?;
    stream.play()?;

    // Load some default sounds for easier testing
    for (i, path) in vec![
        "sounds/kick.wav",
        "sounds/snare.wav",
        "sounds/hihat-open.wav",
        "sounds/hihat-closed.wav",
        "sounds/chord.wav",
        "sounds/bass.wav",
    ]
    .iter()
    .enumerate()
    {
        app.send(Msg::LoadSound(i, Utf8PathBuf::from(path)))?;
    }

    app.run()
}

fn run_audio(mut engine: Engine) -> Result<cpal::Stream> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("can't find output device"))?;

    let mut config = device.default_output_config()?.config();
    config.sample_rate = cpal::SampleRate(SAMPLE_RATE as u32);
    config.buffer_size = cpal::BufferSize::Fixed(FRAMES_PER_BUFFER);
    config.channels = 2;

    // Allocate buffer size x 2, because sometimes cpal requests more than the
    // configured buffer size when switching the output device.
    let mut buf = [(0.0, 0.0); 2 * FRAMES_PER_BUFFER as usize];

    let stream = device.build_output_stream(
        &config,
        move |output: &mut [f32], _: &cpal::OutputCallbackInfo| {
            assert_no_alloc(|| {
                let buf_size = output.len() / 2;
                engine.render(&mut buf[..buf_size]);
                let mut i = 0;
                for frame in &mut buf[..buf_size] {
                    output[i] = frame.0;
                    output[i + 1] = frame.1;
                    i += 2;
                    *frame = (0.0, 0.0);
                }
            });
        },
        move |err| eprintln!("error while processing audio {}", err),
    )?;

    Ok(stream)
}
