extern crate portaudio;

use portaudio as pa;
use vibe::{seq, synth};
use vibe::{FRAMES_PER_BUFFER, PPQN, SAMPLE_RATE};

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
    let mut synth = synth::Synth::new();
    let mut seq = seq::Sequencer::new();

    let pattern = [64, 0, 59, 0, 62, 0, 64, 0, 62, 0, 59, 0, 57, 0, 59, 0];
    let sixteenth = (PPQN / 4) as i32;
    for (step, pitch) in pattern.iter().enumerate() {
        if *pitch > 0 {
            let duration = sixteenth;
            seq.add_note(*pitch - 24 as i32, step as i32 * sixteenth, duration);
        }
    }
    let pa = pa::PortAudio::new()?;

    let mut settings =
        pa.default_output_stream_settings::<f32>(2, SAMPLE_RATE, FRAMES_PER_BUFFER)?;
    settings.flags = pa::stream_flags::CLIP_OFF;

    let mut buf = [0.; FRAMES_PER_BUFFER as usize];

    let callback = move |pa::OutputStreamCallbackArgs { buffer, frames, .. }| {
        let mut block = seq::Block { start: 0, end: 0 };
        while seq.next_block(&mut block, frames, &mut synth) {
            synth.render(&mut buf[block.start..block.end]);
        }

        let mut i = 0;
        for j in 0..buf.len() {
            let sample = buf[j] * 0.15;
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
