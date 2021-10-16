extern crate anyhow;
extern crate atomic_float;

#[macro_use]
extern crate lazy_static;

mod app;
mod engine;
mod env;
mod input;
mod param;
mod pattern;
mod sampler;
mod ui;

use anyhow::{anyhow, Result};
use app::{Action, App, AppCommand};
use camino::Utf8PathBuf;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use engine::{Engine, EngineCommand, EngineParams};
use ringbuf::RingBuffer;

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
    let (engine_send, engine_rcv) = RingBuffer::<EngineCommand>::new(16).split();
    let (app_send, app_recv) = RingBuffer::<AppCommand>::new(16).split();

    let params = EngineParams::default();
    let engine = Engine::new(params.clone(), engine_rcv, app_send);
    let mut app = App::new(params, app_recv, engine_send)?;
    let stream = start_audio(engine)?;
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
        app.take(Action::LoadSound(i, Utf8PathBuf::from(path)))?;
    }
    app.run()
}

fn start_audio(mut engine: Engine) -> Result<cpal::Stream> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(anyhow!("can't find output device"))?;

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
            let buf_size = output.len() / 2;
            engine.render(&mut buf[..buf_size]);
            let mut i = 0;
            for frame in &mut buf[..buf_size] {
                output[i] = frame.0;
                output[i + 1] = frame.1;
                i += 2;
                *frame = (0.0, 0.0);
            }
        },
        move |err| eprintln!("error while processing audio {}", err),
    )?;

    Ok(stream)
}
