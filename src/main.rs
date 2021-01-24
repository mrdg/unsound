extern crate anyhow;
extern crate portaudio;

mod app;
mod env;
mod host;
mod param;
mod sampler;
mod seq;
mod ui;

use anyhow::Result;
use app::Action;
use host::Host;
use ui::Ui;

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
    let (host_state, mut ui_state) = app::new();

    // Load some default sounds for easier testing
    ui_state.take(Action::AddTrack(String::from("sounds/kick.wav")))?;
    ui_state.take(Action::AddTrack(String::from("sounds/snare.wav")))?;
    ui_state.take(Action::AddTrack(String::from("sounds/hihat.wav")))?;
    ui_state.take(Action::AddTrack(String::from("sounds/chord.wav")))?;
    ui_state.take(Action::AddTrack(String::from("sounds/bass.wav")))?;

    let mut host = Host::run(host_state)?;
    let result = Ui::new(ui_state)?.run();
    host.shutdown()?;
    result
}
