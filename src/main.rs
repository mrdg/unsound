extern crate anyhow;

#[macro_use]
extern crate lazy_static;

mod app;
mod audio;
mod engine;
mod env;
mod files;
mod input;
mod pattern;
mod sampler;
mod view;

use anyhow::{anyhow, Result};
use app::{Msg, TrackType};
use assert_no_alloc::*;
use audio::Stereo;
use camino::Utf8PathBuf;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use engine::{Engine, INSTRUMENT_TRACKS};
use ringbuf::RingBuffer;
use triple_buffer::{Output, TripleBuffer};

use crate::{
    app::{App, AppState},
    engine::EngineCommand,
};

#[cfg(debug_assertions)]
#[global_allocator]
static A: AllocDisabler = AllocDisabler;

// Keep https://github.com/RustAudio/cpal/issues/508 in mind
// when changing the sample rate.
const SAMPLE_RATE: f64 = 44100.0;
const FRAMES_PER_BUFFER: usize = 256;

// Allocate buffer size x 2, because sometimes cpal requests more than the
// configured buffer size when switching the output device.
const INTERNAL_BUFFER_SIZE: usize = 2 * FRAMES_PER_BUFFER;

fn main() {
    match run() {
        Ok(_) => {}
        err => {
            eprintln!("error: {:?}", err);
        }
    }
}

fn run() -> Result<()> {
    let (app_state, engine_state) = app::new()?;

    let (app_input, app_output) = TripleBuffer::new(&app_state).split();
    let (engine_input, engine_output) = TripleBuffer::new(&engine_state).split();

    let (producer, consumer) = RingBuffer::<EngineCommand>::new(64).split();

    let engine = Engine::new(engine_state, engine_input, consumer);
    let mut app = App::new(app_state, app_input, engine_output, producer)?;

    for _ in 0..8 {
        app.send(Msg::CreatePattern(None))?
    }

    let num_tracks = INSTRUMENT_TRACKS;

    // Load some default sounds for easier testing
    let sounds = vec![
        "sounds/kick.wav",
        "sounds/snare.wav",
        "sounds/hihat-open.wav",
        "sounds/hihat-closed.wav",
        "sounds/chord.wav",
        "sounds/bass.wav",
    ];
    for i in 0..num_tracks {
        app.send(Msg::CreateTrack(i, TrackType::Instrument))?;
        if i < sounds.len() {
            app.send(Msg::LoadSound(i, Utf8PathBuf::from(sounds[i])))?;
        }
    }
    app.send(Msg::CreateTrack(num_tracks, TrackType::Master))?;
    app.update_state();

    let stream = run_audio(app_output, engine)?;
    stream.play()?;

    app.run()
}

fn run_audio(mut app_state_buf: Output<AppState>, mut engine: Engine) -> Result<cpal::Stream> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("can't find output device"))?;

    let mut config = device.default_output_config()?.config();
    config.sample_rate = cpal::SampleRate(SAMPLE_RATE as u32);
    config.buffer_size = cpal::BufferSize::Fixed(FRAMES_PER_BUFFER as u32);
    config.channels = 2;

    let mut buf = [Stereo::ZERO; INTERNAL_BUFFER_SIZE];
    let stream = device.build_output_stream(
        &config,
        move |output: &mut [f32], _: &cpal::OutputCallbackInfo| {
            assert_no_alloc(|| {
                let buf_size = output.len() / 2;
                engine.render(app_state_buf.read(), &mut buf[..buf_size]);
                let mut i = 0;
                for frame in &mut buf[..buf_size] {
                    output[i] = frame.channel(0);
                    output[i + 1] = frame.channel(1);
                    i += 2;
                    *frame = Stereo::ZERO;
                }
            });
        },
        move |err| eprintln!("error while processing audio {}", err),
    )?;

    Ok(stream)
}
