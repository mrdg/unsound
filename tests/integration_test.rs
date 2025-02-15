use std::fs;
use std::path;

use anyhow::Result;
use camino::Utf8Path;
use hound::{WavReader, WavSpec, WavWriter};

use unsound::app::{self, Msg, TrackType};
use unsound::audio::Stereo;
use unsound::engine::{MAIN_OUTPUT, MASTER_TRACK};
use unsound::pattern::Position;

#[test]
fn test_app() -> Result<()> {
    use Msg::*;
    let (mut app, mut app_state, mut engine, _) = app::new()?;

    let messages = vec![
        SetBpm(120),
        CreateTrack(MASTER_TRACK, MAIN_OUTPUT, TrackType::Bus, None),
        CreateTrack(0, MASTER_TRACK, TrackType::Instrument, None),
        LoadSound(0, "sounds/kick.wav".into()),
        CreatePattern(None),
        TogglePlay,
    ];
    for msg in messages {
        app.send(msg)?;
    }

    let mut cursor = Position::default();
    app.send(app.update_pattern(|p| {
        p.set_len(16);
        for _ in 0..4 {
            p.set_key(cursor, 4, 'z');
            cursor.line += 4 // lines per beat;
        }
    }))?;

    let spec = WavSpec {
        channels: 2,
        sample_rate: 44100,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let output_dir = Utf8Path::new("tests/output");
    fs::remove_dir_all(output_dir)?;
    fs::create_dir_all(output_dir)?;

    let output_file = output_dir.join("test.wav");
    let mut wav = WavWriter::create(&output_file, spec)?;

    let mut output = vec![Stereo::ZERO; 2 * spec.sample_rate as usize];
    let buf_size = 512;
    let mut offset = 0;
    while offset < output.len() {
        let remaining = usize::min(buf_size, output.len() - offset);
        let buf = &mut output[offset..offset + remaining];
        engine.process(app_state.read(), buf);
        offset += buf_size;
        for frame in buf {
            wav.write_sample(frame.channel(0))?;
            wav.write_sample(frame.channel(1))?;
        }
    }
    wav.finalize()?;

    let reference = Utf8Path::new("tests/data/output.wav");
    compare_wav_files(reference, &output_file)?;

    Ok(())
}

fn compare_wav_files<P: AsRef<path::Path>>(left: P, right: P) -> Result<()> {
    let mut reader1 = WavReader::open(left)?;
    let mut reader2 = WavReader::open(right)?;
    assert_eq!(reader1.len(), reader2.len());

    let spec1 = reader1.spec();
    let spec2 = reader2.spec();

    assert_eq!(spec1.channels, spec2.channels);
    assert_eq!(spec1.sample_rate, spec2.sample_rate);
    assert_eq!(spec1.bits_per_sample, spec2.bits_per_sample);

    for (sample1, sample2) in reader1.samples().zip(reader2.samples()) {
        let sample1: f32 = sample1?;
        let sample2: f32 = sample2?;
        assert_eq!(sample1, sample2);
    }
    Ok(())
}
