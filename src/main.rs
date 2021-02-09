extern crate anyhow;
extern crate atomic_float;
extern crate portaudio;

#[macro_use]
extern crate lazy_static;

mod app;
mod env;
mod host;
mod input;
mod param;
mod sampler;
mod seq;
mod ui;

use anyhow::Result;
use app::{Action, App, AppCommand};
use host::{Host, HostCommand, HostParams};
use portaudio::stream_flags as paflags;
use portaudio::OutputStreamCallbackArgs;
use portaudio::PortAudio;
use ringbuf::RingBuffer;

const SAMPLE_RATE: f64 = 44_100.0;
const FRAMES_PER_BUFFER: u32 = 64;

fn main() {
    match run() {
        Ok(_) => {}
        err => {
            eprintln!("error: {:?}", err);
        }
    }
}

fn run() -> Result<()> {
    let (host_send, host_recv) = RingBuffer::<HostCommand>::new(16).split();
    let (app_send, app_recv) = RingBuffer::<AppCommand>::new(16).split();

    let params = HostParams::default();
    let host = Host::new(params.clone(), host_recv, app_send);
    let mut app = App::new(params, app_recv, host_send)?;
    let mut stream = run_audio(host)?;

    // Load some default sounds for easier testing
    app.take(Action::AddTrack(String::from("sounds/kick.wav")))?;
    app.take(Action::AddTrack(String::from("sounds/snare.wav")))?;
    app.take(Action::AddTrack(String::from("sounds/hihat.wav")))?;
    app.take(Action::AddTrack(String::from("sounds/chord.wav")))?;
    app.take(Action::AddTrack(String::from("sounds/bass.wav")))?;

    let result = app.run();

    stream.stop()?;
    stream.close()?;

    result
}

type AudioStream = portaudio::Stream<portaudio::NonBlocking, portaudio::Output<f32>>;

fn run_audio(mut host: Host) -> Result<AudioStream> {
    let pa = PortAudio::new()?;
    let mut settings =
        pa.default_output_stream_settings::<f32>(2, SAMPLE_RATE, FRAMES_PER_BUFFER)?;
    settings.flags = paflags::CLIP_OFF;

    let mut buf = [(0., 0.); FRAMES_PER_BUFFER as usize];
    let callback = move |OutputStreamCallbackArgs { buffer, .. }| {
        host.render(&mut buf);

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
