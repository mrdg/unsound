extern crate anyhow;

use std::{sync::mpsc, thread, time::Duration};

use anyhow::{anyhow, Result};
use camino::Utf8PathBuf;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;
use ringbuf::{Consumer, RingBuffer};
use triple_buffer::{self, TripleBuffer};

use unsound::app::{App, AppCommand, AppState, EngineState, Msg, TrackType};
use unsound::audio::Stereo;
use unsound::engine::{Engine, EngineCommand};
use unsound::files::FileBrowser;
use unsound::input;
use unsound::view::{self, View};

fn main() {
    match run() {
        Ok(_) => {}
        err => {
            eprintln!("error: {:?}", err);
        }
    }
}

fn run() -> Result<()> {
    let app_state = AppState::default();
    let engine_state = EngineState::default();

    let (app_state_input, app_state_output) = TripleBuffer::new(&app_state).split();
    let (engine_state_input, engine_state_output) = TripleBuffer::new(&engine_state).split();

    let (eng_prod, eng_cons) = RingBuffer::<EngineCommand>::new(64).split();
    let (app_prod, app_cons) = RingBuffer::<AppCommand>::new(64).split();

    let file_browser = FileBrowser::with_path("./sounds")?;

    let mut app = App::new(app_state, app_state_input, eng_prod, file_browser);
    let engine = Engine::new(engine_state, engine_state_input, eng_cons, app_prod);

    // Load some default sounds for easier testing
    let sounds = [
        "sounds/kick.wav",
        "sounds/snare.wav",
        "sounds/hihat-open.wav",
        "sounds/hihat-closed.wav",
        "sounds/chord.wav",
        "sounds/bass.wav",
    ];
    for (i, sound) in sounds.iter().enumerate() {
        app.send(Msg::CreateTrack(i, None, TrackType::Instrument, None))?;
        app.send(Msg::LoadSound(i, Utf8PathBuf::from(sound)))?;
    }
    app.send(Msg::LoadEffect(3, "delay".to_string()))?;

    for _ in 0..8 {
        app.send(Msg::CreatePattern(None))?
    }

    let stream = run_audio(app_state_output, engine)?;
    stream.play()?;

    let terminal = ratatui::init();

    let result = run_app(app, engine_state_output, terminal, app_cons);
    ratatui::restore();
    result
}

fn run_audio(
    mut app_state: triple_buffer::Output<AppState>,
    mut engine: Engine,
) -> Result<cpal::Stream> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("can't find output device"))?;

    let mut config = device.default_output_config()?.config();
    config.sample_rate = cpal::SampleRate(unsound::SAMPLE_RATE as u32);
    config.buffer_size = cpal::BufferSize::Fixed(unsound::FRAMES_PER_BUFFER as u32);
    config.channels = 2;

    let mut buf = [Stereo::ZERO; unsound::INTERNAL_BUFFER_SIZE];
    let stream = device.build_output_stream(
        &config,
        move |output: &mut [f32], _: &cpal::OutputCallbackInfo| {
            let buf_size = output.len() / 2;
            engine.process(app_state.read(), &mut buf[..buf_size]);
            let mut i = 0;
            for frame in &mut buf[..buf_size] {
                output[i] = frame.channel(0);
                output[i + 1] = frame.channel(1);
                i += 2;
                *frame = Stereo::ZERO;
            }
        },
        move |err| eprintln!("error while processing audio {}", err),
        None,
    )?;

    Ok(stream)
}

fn run_app(
    mut app: App,
    mut engine_state_handle: triple_buffer::Output<EngineState>,
    mut terminal: DefaultTerminal,
    mut app_cmd_cons: Consumer<AppCommand>,
) -> Result<()> {
    let mut view = View::new();

    let (sender, receiver) = mpsc::channel();
    {
        let sender = sender.clone();
        thread::spawn(move || loop {
            let event = event::read().expect("event read");
            sender
                .send(AppEvent::Input(event))
                .expect("send keyboard input");
        })
    };
    thread::spawn(move || loop {
        sender.send(AppEvent::Draw).expect("sending draw event");
        thread::sleep(Duration::from_millis(33));
    });

    loop {
        let engine_state = engine_state_handle.read();
        app.engine_state.clone_from(engine_state);
        while let Some(cmd) = app_cmd_cons.pop() {
            app.send(Msg::Command(cmd))?;
        }
        terminal.draw(|f| view::render(&app, &mut view, f))?;

        match receiver.recv()? {
            AppEvent::Input(event) => match event {
                Event::Key(event) if event.kind == KeyEventKind::Press => {
                    let msg = input::handle_key_event(&app, &mut view, event);
                    if msg.is_exit() {
                        return Ok(());
                    }
                    if !msg.is_noop() {
                        app.send(msg)?;
                    }
                }
                _ => {}
            },
            AppEvent::Draw => {}
        }
    }
}

enum AppEvent {
    Input(Event),
    Draw,
}
