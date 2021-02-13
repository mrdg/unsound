extern crate anyhow;
extern crate atomic_float;
extern crate portaudio;

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

use anyhow::Result;
use app::{Action, App, AppCommand};
use camino::Utf8PathBuf;
use engine::{Engine, EngineCommand, EngineParams};
use portaudio::stream_flags as paflags;
use portaudio::OutputStreamCallbackArgs;
use portaudio::PortAudio;
use ringbuf::RingBuffer;

const SAMPLE_RATE: f64 = 44_100.0;
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
    let mut stream = run_audio(engine)?;

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

    let result = app.run();

    stream.stop()?;
    stream.close()?;

    result
}

type AudioStream = portaudio::Stream<portaudio::NonBlocking, portaudio::Output<f32>>;

fn run_audio(mut engine: Engine) -> Result<AudioStream> {
    let pa = PortAudio::new()?;
    let mut settings =
        pa.default_output_stream_settings::<f32>(2, SAMPLE_RATE, FRAMES_PER_BUFFER)?;
    settings.flags = paflags::CLIP_OFF;

    let mut buf = [(0., 0.); FRAMES_PER_BUFFER as usize];
    let callback = move |OutputStreamCallbackArgs { buffer, .. }| {
        engine.render(&mut buf);

        let mut i = 0;
        for j in 0..buf.len() {
            buffer[i] = buf[j].0;
            buffer[i + 1] = buf[j].1;
            i += 2;
            buf[j] = (0.0, 0.0);
        }

        portaudio::Continue
    };

    let mut stream = pa.open_non_blocking_stream(settings, callback)?;
    stream.start()?;
    Ok(stream)
}
