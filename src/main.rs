extern crate portaudio;

use portaudio as pa;
use vibe::sampler::{Sampler, Sound};
use vibe::seq::{Block, Pattern, Sequencer};
use vibe::synth::Synth;
use vibe::Instrument;
use vibe::{FRAMES_PER_BUFFER, SAMPLE_RATE};
mod vibe;

fn main() {
    match run() {
        Ok(_) => {}
        e => {
            eprintln!("error: {:?}", e);
        }
    }
}

fn run() -> Result<(), pa::Error> {
    const SD: i32 = 60;
    const HH: i32 = 61;

    let sounds = vec![
        Sound::load(String::from("sounds/snare.wav"), SD),
        Sound::load(String::from("sounds/hihat.wav"), HH),
    ];

    let mut instruments: Vec<Box<dyn Instrument>> =
        vec![Box::new(Sampler::new(sounds)), Box::new(Synth::new())];

    let beat = Pattern::new(
        0,
        vec![HH, 0, HH, 0, SD, 0, HH, 0, HH, 0, HH, 0, SD, 0, HH, 0],
    );
    let bass = Pattern::new(
        1,
        vec![40, 0, 35, 0, 38, 0, 40, 0, 38, 0, 35, 0, 33, 0, 35, 0],
    );

    let mut seq = Sequencer::new(vec![beat, bass]);

    let pa = pa::PortAudio::new()?;
    let channels = 2;
    let mut settings =
        pa.default_output_stream_settings::<f32>(channels, SAMPLE_RATE, FRAMES_PER_BUFFER)?;
    settings.flags = pa::stream_flags::CLIP_OFF;

    let mut buf = [0.; FRAMES_PER_BUFFER as usize];
    let callback = move |pa::OutputStreamCallbackArgs { buffer, frames, .. }| {
        let mut block = Block { start: 0, end: 0 };
        while seq.next_block(&mut block, frames, &mut instruments) {
            for instrument in &mut instruments {
                instrument.render(&mut buf[block.start..block.end]);
            }
        }

        let mut i = 0;
        for j in 0..buf.len() {
            let sample = buf[j] * 0.1;
            buffer[i] = sample;
            buffer[i + 1] = sample;
            i += 2;
            buf[j] = 0.0;
        }
        pa::Continue
    };

    let mut stream = pa.open_non_blocking_stream(settings, callback)?;

    stream.start()?;
    pa.sleep(5000);
    stream.stop()?;
    stream.close()?;

    Ok(())
}
