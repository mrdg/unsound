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

use std::{
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use anyhow::{anyhow, Result};
use assert_no_alloc::*;
use camino::Utf8PathBuf;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;
use triple_buffer::Output;

use crate::app::{App, AppState, EngineState, Msg};
use crate::audio::Stereo;
use crate::engine::{Engine, INSTRUMENT_TRACKS};
use crate::view::View;

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
    let sounds = [
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

    let terminal = ratatui::init();

    let result = run_app(app, engine_state, terminal);
    ratatui::restore();
    result
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

fn run_app(
    mut app: App,
    mut engine_state_handle: Output<EngineState>,
    mut terminal: DefaultTerminal,
) -> Result<()> {
    let mut view = View::new();
    let input = read_input_events();

    loop {
        let engine_state = engine_state_handle.read();
        app.engine_state.clone_from(engine_state);
        terminal.draw(|f| view::render(&app, &mut view, f))?;

        match input.recv()? {
            Input::Event(event) => match event {
                Event::Key(event) if event.kind == KeyEventKind::Press => {
                    let msg = input::handle_key_event(&app, &mut view, event);
                    if msg.is_exit() {
                        return Ok(());
                    }
                    app.send(msg)?;
                }
                _ => {}
            },
            Input::Tick => {}
        }
    }
}

pub enum Input {
    Event(Event),
    Tick,
}

fn read_input_events() -> Receiver<Input> {
    let (sender, receiver) = mpsc::channel();
    {
        let sender = sender.clone();
        thread::spawn(move || loop {
            let event = event::read().expect("event read");
            sender
                .send(Input::Event(event))
                .expect("send keyboard input");
        })
    };
    thread::spawn(move || loop {
        if sender.send(Input::Tick).is_err() {
            return;
        }
        thread::sleep(Duration::from_millis(33));
    });

    receiver
}
