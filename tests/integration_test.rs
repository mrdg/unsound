use std::fs::{self, File};
use std::io::{BufWriter, ErrorKind};
use std::path;
use std::time::Duration;

use anyhow::Result;
use assert_no_alloc::*;
use camino::{Utf8Path, Utf8PathBuf};
use hound::{WavReader, WavSpec, WavWriter};
use ringbuf::{Consumer, RingBuffer};
use triple_buffer::{self, TripleBuffer};

use unsound::app::{App, AppCommand, AppState, EngineState, Msg, TrackType};
use unsound::audio::Stereo;
use unsound::audio_graph;
use unsound::delay::DelayParams;
use unsound::engine::{Engine, EngineCommand};
use unsound::files::FileBrowser;
use unsound::pattern::Position;

#[global_allocator]
static A: AllocDisabler = AllocDisabler;

struct Test {
    app: App,
    app_consumer: Consumer<AppCommand>,
    app_state: triple_buffer::Output<AppState>,
    engine_state: triple_buffer::Output<EngineState>,
    engine: Engine,
}

impl Test {
    fn process(&mut self, buffer: &mut [Stereo]) -> Result<()> {
        assert_no_alloc(|| {
            self.engine.process(self.app_state.read(), buffer);
        });
        self.app.engine_state.clone_from(self.engine_state.read());
        while let Some(cmd) = self.app_consumer.pop() {
            self.app.send(Msg::Command(cmd))?;
        }
        Ok(())
    }

    fn send(&mut self, messages: Vec<Msg>) -> Result<()> {
        for msg in messages {
            self.app.send(msg)?;
        }
        Ok(())
    }
}

fn test_setup() -> Result<Test> {
    let app_state = AppState::default();
    let engine_state = EngineState::default();

    let (app_state_input, app_state_output) = TripleBuffer::new(&app_state).split();
    let (engine_state_input, engine_state_output) = TripleBuffer::new(&engine_state).split();

    let (eng_prod, eng_cons) = RingBuffer::<EngineCommand>::new(64).split();
    let (app_prod, app_cons) = RingBuffer::<AppCommand>::new(64).split();

    let file_browser = FileBrowser::with_path("./sounds")?;

    let engine = Engine::new(engine_state, engine_state_input, eng_cons, app_prod);
    let mut app = App::new(app_state, app_state_input, eng_prod, file_browser);

    let test = Test {
        app,
        app_consumer: app_cons,
        app_state: app_state_output,
        engine,
        engine_state: engine_state_output,
    };
    Ok(test)
}

#[test]
fn test_play_pattern() -> Result<()> {
    use Msg::*;
    let mut test = test_setup()?;
    test.send(vec![
        SetBpm(120),
        CreateTrack(0, None, TrackType::Instrument, None),
        LoadSound(0, "sounds/kick.wav".into()),
        CreatePattern(None),
        TogglePlay,
    ])?;

    let mut cursor = Position::default();
    test.app.send(test.app.update_pattern(|p| {
        p.set_len(16);
        for _ in 0..4 {
            p.handle_input(cursor, 4, 'z', 0);
            cursor.line += 4 // lines per beat;
        }
    }))?;

    let mut rec = Recording::new("play_pattern", Some(44100 * 2))?;
    for mut buf in rec.iter() {
        test.process(&mut buf)?;
        rec.write(&buf)?;
    }
    let output = rec.finish()?;

    let reference = Utf8Path::new("tests/data/play_pattern.wav");
    compare_wav_files(reference, &output)?;

    Ok(())
}

#[test]
fn test_delete_active_track() -> Result<()> {
    use Msg::*;
    let mut test = test_setup()?;

    test.send(vec![
        SetBpm(120),
        CreateTrack(0, None, TrackType::Instrument, None),
        LoadSound(0, "sounds/hihat-open.wav".into()),
        LoadEffect(0, "delay".into()),
        CreatePattern(None),
    ])?;

    // Load an effect to test that deleting it works as expected, but use only the dry
    // signal so it doesn't affect the audio.
    let effect_id = test.app.tracks[0].effects[0].node_id;
    test.send(vec![
        ParamSet(effect_id, DelayParams::WET_MIX, 0.0),
        ParamSet(effect_id, DelayParams::DRY_MIX, 1.0),
        TogglePlay,
    ])?;

    let mut cursor = Position::default();
    test.app.send(test.app.update_pattern(|p| {
        p.set_len(16);
        for _ in 0..p.len() {
            p.handle_input(cursor, 4, 'z', 0);
            cursor.line += 1 // lines per beat;
        }
    }))?;

    let mut rec = Recording::new("delete_active_track", Some(44100 * 2))?;
    let mut samples = 0;
    for mut buf in rec.iter() {
        samples += buf.len();
        if !test.app.tracks.is_empty() && samples >= 44100 {
            test.app.send(Msg::DeleteTrack(0))?;
        }

        test.process(&mut buf)?;
        rec.write(&buf)?;
    }
    let output = rec.finish()?;

    let reference = Utf8Path::new("tests/data/delete_active_track.wav");
    compare_wav_files(reference, &output)?;

    Ok(())
}

#[test]
fn test_node_gc() -> Result<()> {
    use Msg::*;
    let mut test = test_setup()?;
    test.send(vec![
        SetBpm(120),
        CreateTrack(0, None, TrackType::Instrument, None),
        LoadSound(0, "sounds/kick.wav".into()),
        CreatePattern(None),
        TogglePlay,
    ])?;

    let mut cursor = Position::default();
    test.app.send(test.app.update_pattern(|p| {
        p.set_len(16);
        for _ in 0..4 {
            p.handle_input(cursor, 4, 'z', 0);
            cursor.line += 4
        }
    }))?;

    let mut rec = Recording::new("node_gc", None)?;
    let mut i: usize = 1;
    for mut buf in rec.iter() {
        test.process(&mut buf)?;
        rec.write(&buf)?;

        let dur = rec.duration();
        if dur.as_secs() > i as u64 {
            i += 1;
            if i >= 2 * audio_graph::DEFAULT_SIZE {
                break;
            }
            test.app.send(LoadSound(0, "sounds/kick.wav".into()))?;
        }
    }
    rec.finish()?;

    Ok(())
}

struct Recording {
    length: Option<usize>,
    wav: WavWriter<BufWriter<File>>,
    path: Utf8PathBuf,
}

impl Recording {
    fn new(name: &str, length: Option<usize>) -> Result<Self> {
        let output_dir = Utf8Path::new("tests/output");
        fs::create_dir_all(output_dir)?;
        let path = output_dir.join(name).with_extension("wav");
        match fs::remove_file(&path) {
            Err(err) if err.kind() != ErrorKind::NotFound => return Err(err.into()),
            _ => {}
        }

        let spec = WavSpec {
            channels: 2,
            sample_rate: 44100,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let wav = WavWriter::create(&path, spec)?;
        Ok(Self { wav, length, path })
    }

    fn write(&mut self, buf: &[Stereo]) -> Result<()> {
        for frame in buf {
            self.wav.write_sample(frame.channel(0))?;
            self.wav.write_sample(frame.channel(1))?;
        }
        Ok(())
    }

    fn duration(&self) -> Duration {
        let secs = self.wav.duration() / self.wav.spec().sample_rate;
        Duration::from_secs(secs.into())
    }

    fn iter(&self) -> impl Iterator<Item = Vec<Stereo>> {
        RecordingIter {
            buffer_size: 512,
            position: 0,
            length: self.length.unwrap_or(usize::MAX),
        }
    }

    fn finish(self) -> Result<Utf8PathBuf> {
        self.wav.finalize()?;
        Ok(self.path)
    }
}

struct RecordingIter {
    buffer_size: usize,
    position: usize,
    length: usize,
}

impl Iterator for RecordingIter {
    type Item = Vec<Stereo>;

    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.length - self.position;
        if remaining > 0 {
            let buf_size = usize::min(remaining, self.buffer_size);
            self.position += buf_size;
            vec![Stereo::ZERO; buf_size].into()
        } else {
            None
        }
    }
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
