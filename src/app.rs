use anyhow::{anyhow, Result};
use atomic_float::AtomicF32;
use camino::Utf8PathBuf;
use ringbuf::{Producer, RingBuffer};
use triple_buffer::{Input, Output, TripleBuffer};

use crate::audio::Stereo;
use crate::engine::{self, Device as AudioDevice, Engine, INSTRUMENT_TRACKS};
use crate::files::FileBrowser;
use crate::params::Params;
use crate::pattern::{Position, Step, StepSize, MAX_PATTERNS};
use crate::sampler::{self, AudioFile, Sampler};
use crate::{engine::EngineCommand, pattern::Pattern, sampler::Sound};
use std::collections::HashMap;
use std::sync::Arc;

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::sync::atomic::Ordering;

pub struct App {
    pub state: AppState,
    state_buf: Input<AppState>,
    producer: Producer<EngineCommand>,
    pub file_browser: FileBrowser,
    file_cache: HashMap<Utf8PathBuf, Arc<AudioFile>>,
    id_generator: IdGenerator,
    pub device_params: HashMap<DeviceID, Arc<dyn Params>>,
}

impl App {
    pub fn send(&mut self, msg: Msg) -> Result<()> {
        self.dispatch(msg)?;
        let input_buf = self.state_buf.input_buffer();
        input_buf.clone_from(&self.state);
        self.state_buf.publish();
        Ok(())
    }

    fn dispatch(&mut self, msg: Msg) -> Result<()> {
        use Msg::*;
        match msg {
            Noop => {}
            Exit => {}
            TogglePlay => {
                self.state.is_playing = !self.state.is_playing;
            }
            SetPatternStep(pos, step) => self.update_pattern(|p| p.set_step(pos, step)),
            SetBpm(bpm) => self.state.bpm = bpm,
            SetOct(oct) => self.state.octave = oct,
            LoadSound(idx, path) => {
                let snd = self.load_sound(path)?;
                self.state.sounds[idx] = Some(snd);
            }
            LoopToggle(idx) => {
                self.state.loop_range = match self.state.loop_range {
                    Some((start, end)) => {
                        if start == idx && idx == end {
                            None
                        } else {
                            Some((idx, idx))
                        }
                    }
                    None => Some((idx, idx)),
                }
            }
            LoopAdd(idx) => {
                self.state.loop_range = match self.state.loop_range {
                    Some((start, end)) => {
                        if idx < start {
                            Some((idx, end))
                        } else {
                            Some((start, idx))
                        }
                    }
                    None => Some((idx, idx)),
                }
            }
            PreviewSound(path) => {
                let snd = self.load_sound(path)?;
                self.send_to_engine(EngineCommand::PreviewSound(snd))?;
            }
            SelectPattern(idx) => {
                if idx < self.state.song.len() {
                    self.state.selected_pattern = idx;
                }
            }
            NextPattern => {
                self.state.selected_pattern =
                    usize::min(self.state.selected_pattern + 1, self.state.song.len() - 1)
            }
            PrevPattern => {
                self.state.selected_pattern = self.state.selected_pattern.saturating_sub(1);
            }
            DeletePattern(idx) => {
                // Ensure we have at least one to avoid dealing with having no patterns
                if self.state.song.len() > 1 {
                    let pattern_id = self.state.song.remove(idx);
                    if !self.state.song.contains(&pattern_id) {
                        self.state.patterns.remove(&pattern_id);
                    }
                    if self.state.selected_pattern >= self.state.song.len() {
                        self.state.selected_pattern = self.state.selected_pattern.saturating_sub(1);
                    }
                    // Ensure that loop start and end are in bounds with respect to song vector
                    if let Some(loop_range) = &mut self.state.loop_range {
                        let end = self.state.song.len() - 1;
                        *loop_range = (usize::min(loop_range.0, end), usize::min(loop_range.1, end))
                    }
                }
            }
            CreatePattern(idx) => {
                if self.state.patterns.len() < MAX_PATTERNS {
                    let id = self.next_pattern_id();
                    let num_instruments = self
                        .state
                        .tracks
                        .iter()
                        .filter(|track| matches!(track.track_type, TrackType::Instrument))
                        .count();

                    let pattern = Pattern::new(num_instruments);
                    self.state.patterns.insert(id, Arc::new(pattern));
                    if let Some(idx) = idx {
                        self.state.song.insert(idx + 1, id);
                    } else {
                        self.state.song.push(id);
                    }
                }
            }
            RepeatPattern(idx) => {
                let pattern_id = self.state.song[idx];
                self.state.song.insert(idx + 1, pattern_id);
            }
            ClonePattern(idx) => {
                let pattern_id = self.state.song[idx];
                let clone = self.state.patterns.get(&pattern_id).unwrap().clone();
                let id = self.next_pattern_id();
                self.state.patterns.insert(id, clone);
                self.state.song.insert(idx + 1, id);
            }
            SetPatternLen(len) => self.update_pattern(|p| p.set_len(len)),
            ChangeDir(dir) => self.file_browser.move_to(dir)?,
            ToggleMute(track) => {
                let muted = &mut self.state.tracks[track].muted;
                *muted = !*muted;
            }
            VolumeInc(track) => self.track_volume(track).inc(),
            VolumeDec(track) => self.track_volume(track).dec(),
            SetVolume(track, value) => self.track_volume(track).set(value),
            CreateTrack(idx, track_type) => {
                let track = self.create_track(track_type)?;
                self.state.tracks.insert(idx, track);
            }
            ParamInc(track_idx, device_idx, param_idx, step_size) => {
                let device_id = self.state.tracks[track_idx].devices[device_idx].id;
                let params = self.device_params.get(&device_id).unwrap();
                params.get_param(param_idx).incr(step_size);
            }
            ParamDec(track_idx, device_idx, param_idx, step_size) => {
                let device_id = self.state.tracks[track_idx].devices[device_idx].id;
                let params = self.device_params.get(&device_id).unwrap();
                params.get_param(param_idx).decr(step_size);
            }
        }

        Ok(())
    }

    fn create_track(&mut self, track_type: TrackType) -> Result<Track> {
        let track_id = self.id_generator.track();
        let cmd = EngineCommand::CreateTrack(track_id, Box::new(engine::Track::new()));
        self.send_to_engine(cmd)?;

        let mut devices = Vec::new();
        if matches!(track_type, TrackType::Instrument) {
            let sampler = Box::new(Sampler::new());
            let params = sampler.params();
            let sampler_id = self.id_generator.device();
            self.device_params.insert(sampler_id, params);
            self.send_to_engine(EngineCommand::CreateDevice(track_id, sampler_id, sampler))?;

            let sampler = Device {
                name: "Sampler".to_owned(),
                id: sampler_id,
            };
            devices = vec![sampler];
        }

        let track = Track {
            id: track_id,
            devices,
            muted: false,
            track_type,
            volume: Volume::new(-6.0),
            name: None,
            rms: [Arc::new(AtomicF32::new(0.0)), Arc::new(AtomicF32::new(0.0))],
        };
        Ok(track)
    }

    fn track_volume(&mut self, track: Option<usize>) -> &mut Volume {
        if let Some(track) = track {
            &mut self.state.tracks[track].volume
        } else {
            &mut self.state.tracks.last_mut().unwrap().volume
        }
    }

    fn load_sound(&mut self, path: Utf8PathBuf) -> Result<Sound> {
        let file = if let Some(file) = self.file_cache.get(&path) {
            file.clone()
        } else {
            // Sounds with only a single reference are not loaded in an instrument slot (and
            // so cannot be cloned by the audio thread) so we can safely delete these
            // from the cache.
            self.file_cache.retain(|_, v| Arc::strong_count(v) > 1);

            let file = Arc::new(sampler::load_file(&path)?);
            self.file_cache.insert(path.clone(), file.clone());
            file
        };

        Ok(Sound {
            offset: file.offset,
            path,
            file,
        })
    }

    fn next_pattern_id(&self) -> PatternID {
        if self.state.patterns.is_empty() {
            return PatternID(0);
        }
        let mut max = 0;
        for id in self.state.patterns.keys() {
            if id.0 > max {
                max = id.0;
            }
        }
        PatternID(max + 1)
    }

    fn update_pattern<F>(&mut self, f: F)
    where
        F: Fn(&mut Pattern),
    {
        let id = self.state.song[self.state.selected_pattern];
        if let Some(pattern) = self.state.patterns.get(&id) {
            let mut new = (**pattern).clone();
            f(&mut new);
            self.state.patterns.insert(id, Arc::new(new));
        }
    }

    fn send_to_engine(&mut self, cmd: EngineCommand) -> Result<()> {
        if self.producer.push(cmd).is_err() {
            return Err(anyhow!("unable to send message to engine"));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct EngineState {
    pub current_tick: usize,
    pub current_pattern: usize,
}

#[derive(Clone)]
pub struct AppState {
    pub lines_per_beat: u16,
    pub bpm: u16,
    pub octave: u16,
    pub is_playing: bool,
    pub selected_pattern: usize,
    pub patterns: HashMap<PatternID, Arc<Pattern>>,
    pub song: Vec<PatternID>,
    pub loop_range: Option<(usize, usize)>,
    pub sounds: Vec<Option<Sound>>,
    pub tracks: Vec<Track>,
}

#[derive(Clone)]
pub struct Track {
    pub id: TrackID,
    pub devices: Vec<Device>,

    pub track_type: TrackType,
    pub name: Option<String>,
    pub volume: Volume,
    pub muted: bool,
    rms: [Arc<AtomicF32>; 2],
}

impl Track {
    pub fn rms(&self) -> (f32, f32) {
        (
            self.rms[0].load(Ordering::Relaxed),
            self.rms[1].load(Ordering::Relaxed),
        )
    }

    pub fn update_rms(&self, rms: Stereo) {
        self.rms[0].store(rms.channel(0), Ordering::Relaxed);
        self.rms[1].store(rms.channel(1), Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub struct Device {
    pub id: DeviceID,
    pub name: String,
}

#[derive(Copy, Clone, Debug)]
pub enum TrackType {
    Instrument,
    Bus,
}

pub fn new() -> Result<(App, Output<AppState>, Engine, Output<EngineState>)> {
    let engine_state = EngineState {
        current_pattern: 0,
        current_tick: 0,
    };

    let app_state = AppState {
        bpm: 120,
        lines_per_beat: 4,
        octave: 4,
        is_playing: false,
        patterns: HashMap::new(),
        song: Vec::new(),
        selected_pattern: 0,
        loop_range: Some((0, 0)),
        sounds: vec![None; INSTRUMENT_TRACKS],
        tracks: Vec::new(),
    };

    // Triple buffers are used to share app state with the engine and vice versa. This should
    // ensure that both threads always have a coherent view of the other thread's state.
    let (app_state_input, app_state_output) = TripleBuffer::new(&app_state).split();
    let (engine_state_input, engine_state_output) = TripleBuffer::new(&engine_state).split();

    let (producer, consumer) = RingBuffer::<EngineCommand>::new(64).split();
    let engine = Engine::new(engine_state, engine_state_input, consumer);

    let mut app = App {
        state: app_state,
        state_buf: app_state_input,
        producer,
        file_browser: FileBrowser::with_path("./sounds")?,
        file_cache: HashMap::new(),
        id_generator: IdGenerator { current: 0 },
        device_params: HashMap::new(),
    };

    let mut master_bus = app.create_track(TrackType::Bus)?;
    master_bus.name = Some("Master".to_owned());
    app.state.tracks.push(master_bus);
    Ok((app, app_state_output, engine, engine_state_output))
}

pub enum Msg {
    Noop,
    Exit,
    TogglePlay,
    SetPatternStep(Position, Step),
    LoadSound(usize, Utf8PathBuf),
    PreviewSound(Utf8PathBuf),
    LoopAdd(usize),
    LoopToggle(usize),
    SelectPattern(usize),
    NextPattern,
    PrevPattern,
    DeletePattern(usize),
    CreatePattern(Option<usize>),
    RepeatPattern(usize),
    ClonePattern(usize),
    SetPatternLen(usize),
    ChangeDir(Utf8PathBuf),
    SetBpm(u16),
    SetOct(u16),
    ToggleMute(usize),
    VolumeInc(Option<usize>),
    VolumeDec(Option<usize>),
    SetVolume(Option<usize>, f64),
    CreateTrack(usize, TrackType),
    ParamInc(usize, usize, usize, StepSize),
    ParamDec(usize, usize, usize, StepSize),
}

impl Msg {
    pub fn is_exit(&self) -> bool {
        matches!(self, Self::Exit)
    }
}

pub trait SharedState {
    fn app(&self) -> &AppState;

    fn lines_per_beat(&self) -> u16 {
        self.app().lines_per_beat
    }

    fn bpm(&self) -> u16 {
        self.app().bpm
    }

    fn is_playing(&self) -> bool {
        self.app().is_playing
    }

    fn sounds(&self) -> &Vec<Option<Sound>> {
        &self.app().sounds
    }

    fn tracks(&self) -> &Vec<Track> {
        &self.app().tracks
    }
}

#[derive(Clone, Copy)]
pub struct AudioContext<'a> {
    app_state: &'a AppState,
}

impl<'a> AudioContext<'a> {
    pub fn new(app_state: &'a AppState) -> Self {
        Self { app_state }
    }

    pub fn master_bus(&self) -> &Track {
        // the master track always exists so it's ok to unwrap here
        self.app_state.tracks.last().unwrap()
    }

    pub fn next_pattern(&self, current: usize) -> usize {
        let (start, end) = match self.app_state.loop_range {
            Some(range) => range,
            None => (0, self.app_state.song.len() - 1),
        };
        let mut next = current + 1;
        if next > end {
            next = start;
        }
        next
    }

    pub fn is_track_muted(&self, track: usize) -> bool {
        self.app_state.tracks[track].muted
    }

    pub fn sound(&self, idx: usize) -> &Option<Sound> {
        self.sounds().get(idx).unwrap_or(&None)
    }

    pub fn pattern(&self, idx: usize) -> Option<&Arc<Pattern>> {
        self.app()
            .song
            .get(idx)
            .and_then(|id| self.app().patterns.get(id))
    }
}

impl<'a> SharedState for AudioContext<'a> {
    fn app(&self) -> &AppState {
        self.app_state
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct PatternID(u64);

impl Display for PatternID {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Copy, Clone)]
pub struct Volume {
    value: f64,
    value_db: f64,
}

impl Volume {
    fn new(db: f64) -> Self {
        let mut v = Self {
            value_db: 0.0,
            value: 0.0,
        };
        v.set(db);
        v
    }

    pub fn db(&self) -> f64 {
        self.value_db
    }

    pub fn val(&self) -> f64 {
        self.value
    }

    fn inc(&mut self) {
        self.set(self.value_db + 0.25);
    }

    fn dec(&mut self) {
        self.set(self.value_db - 0.25);
    }

    fn set(&mut self, db: f64) {
        let db = f64::min(db, 3.0);
        let db = f64::max(db, -60.0);
        self.value_db = db;
        self.value = f64::powf(10.0, db / 20.0);
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct DeviceID(u64);

#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct TrackID(u64);

struct IdGenerator {
    current: u64,
}

impl IdGenerator {
    fn device(&mut self) -> DeviceID {
        DeviceID(self.next())
    }

    fn track(&mut self) -> TrackID {
        TrackID(self.next())
    }

    fn next(&mut self) -> u64 {
        let id = self.current;
        self.current += 1;
        id
    }
}
