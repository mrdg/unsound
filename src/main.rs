extern crate anyhow;

#[macro_use]
extern crate lazy_static;

mod app;
mod audio;
mod engine;
mod env;
mod files;
mod input;
mod params;
mod pattern;
mod sampler;
mod view;

use anyhow::{anyhow, Result};
use app::{EngineState, Msg};
use assert_no_alloc::*;
use audio::Stereo;
use camino::Utf8PathBuf;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use engine::{Engine, INSTRUMENT_TRACKS};
use triple_buffer::Output;
use view::{InputQueue, ViewContext};

use crate::view::View;
use std::io;
use termion::{input::MouseTerminal, raw::IntoRawMode, screen::AlternateScreen};
use tui::{backend::TermionBackend, Terminal};

use crate::app::{App, AppState};

#[cfg(debug_assertions)]
#[global_allocator]
static A: AllocDisabler = AllocDisabler;

// Keep https://github.com/RustAudio/cpal/issues/508 in mind
// when changing the sample rate.
const SAMPLE_RATE: f64 = 44100.0;
const FRAMES_PER_BUFFER: usize = 128;

// Allocate a larger buffer size, because sometimes cpal requests more than the
// configured buffer size when switching the output device.
const INTERNAL_BUFFER_SIZE: usize = 4 * FRAMES_PER_BUFFER;

fn main() {
    match run() {
        Ok(_) => {}
        err => {
            eprintln!("error: {:?}", err);
        }
    }
}

fn run() -> Result<()> {
    let (mut app, app_state, engine, engine_state) = app::new()?;

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
        app.send(Msg::CreateTrack(i))?;
        if i < sounds.len() {
            app.send(Msg::LoadSound(i, Utf8PathBuf::from(sounds[i])))?;
        }
    }
    for _ in 0..8 {
        app.send(Msg::CreatePattern(None))?
    }

    let stream = run_audio(app_state, engine)?;
    stream.play()?;

    run_app(app, engine_state)
}

fn run_audio(mut app_state: Output<AppState>, mut engine: Engine) -> Result<cpal::Stream> {
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
                engine.process(app_state.read(), &mut buf[..buf_size]);
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
        None,
    )?;

    Ok(stream)
}

fn run_app(mut app: App, mut engine_state: Output<EngineState>) -> Result<()> {
    let mut input = InputQueue::new();
    let stdout = io::stdout().into_raw_mode()?;
    let stdout = MouseTerminal::from(stdout);
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut view = View::new();
    loop {
        let ctx = ViewContext::new(
            &app.device_params,
            &app.state,
            engine_state.read(),
            &app.file_browser,
        );
        terminal.draw(|f| view.render(f, ctx))?;

        match input.next()? {
            view::Input::Key(key) => {
                let msg = view.handle_input(key, ctx);
                if msg.is_exit() {
                    return Ok(());
                }
                app.send(msg)?;
            }
            view::Input::Tick => {}
        }
    }
}
