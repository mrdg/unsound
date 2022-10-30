use anyhow::{anyhow, Result};
use atomic_float::AtomicF64;
use camino::Utf8PathBuf;
use lru::LruCache;
use ringbuf::{Producer, RingBuffer};
use triple_buffer::{Input, Output, TripleBuffer};
use ulid::Ulid;

use crate::engine::{self, Engine, Plugin, INSTRUMENT_TRACKS};
use crate::files::FileBrowser;
use crate::params::Params;
use crate::pattern::{Position, Step, StepSize, MAX_PATTERNS};
use crate::sampler::{self, Sampler, ROOT_PITCH};
use crate::{engine::EngineCommand, pattern::Pattern};
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
    pub device_params: HashMap<DeviceId, Arc<dyn Params>>,
    preview_cache: LruCache<Utf8PathBuf, DeviceId>,
    preview_track_id: TrackId,
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
                // TODO: keep settings from previous sampler?
                let snd = sampler::load_file(&path)?;
                let sampler = Box::new(Sampler::new(snd));
                let sampler_id = DeviceId::new();
                self.device_params.insert(sampler_id, sampler.params());

                let cmd = EngineCommand::CreateInstrument(sampler_id, sampler);
                self.send_to_engine(cmd)?;

                if let Some(instr) = &self.state.instruments[idx] {
                    self.send_to_engine(EngineCommand::DeleteInstrument(instr.id))?;
                }

                self.state.instruments[idx] = Some(Instrument {
                    id: sampler_id,
                    name: path.file_name().unwrap().to_string(),
                });
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
                let device_id = if let Some(id) = self.preview_cache.get(&path) {
                    *id
                } else {
                    let snd = sampler::load_file(&path)?;
                    let sampler = Box::new(Sampler::new(snd));
                    let sampler_id = DeviceId::new();
                    self.preview_cache.put(path.clone(), sampler_id);
                    self.send_to_engine(EngineCommand::CreateInstrument(sampler_id, sampler))?;
                    sampler_id
                };
                if self.preview_cache.len() > 10 {
                    if let Some((_, device_id)) = self.preview_cache.pop_lru() {
                        self.send_to_engine(EngineCommand::DeleteInstrument(device_id))?;
                    }
                }
                self.send_to_engine(EngineCommand::PlayNote(
                    device_id,
                    self.preview_track_id,
                    ROOT_PITCH,
                ))?;
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
            CreateTrack(idx) => {
                let track = engine::Track::new();
                let volume = Volume::new(-6.0, track.volume.clone());
                let rms = track.rms_out.clone();
                let track_info = Track::new(volume, rms);
                let cmd = EngineCommand::CreateTrack(track_info.id, Box::new(track));
                self.send_to_engine(cmd)?;
                self.state.tracks.insert(idx, track_info);
            }
            ParamInc(device_id, param_idx, step_size) => {
                let params = self.device_params.get(&device_id).unwrap();
                params.get_param(param_idx).incr(step_size);
            }
            ParamDec(device_id, param_idx, step_size) => {
                let params = self.device_params.get(&device_id).unwrap();
                params.get_param(param_idx).decr(step_size);
            }
        }

        Ok(())
    }

    fn track_volume(&mut self, track: Option<usize>) -> &mut Volume {
        if let Some(track) = track {
            &mut self.state.tracks[track].volume
        } else {
            &mut self.state.tracks.last_mut().unwrap().volume
        }
    }

    fn next_pattern_id(&self) -> PatternId {
        if self.state.patterns.is_empty() {
            return PatternId(0);
        }
        let mut max = 0;
        for id in self.state.patterns.keys() {
            if id.0 > max {
                max = id.0;
            }
        }
        PatternId(max + 1)
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
pub struct Instrument {
    pub name: String,
    pub id: DeviceId,
}

#[derive(Clone)]
pub struct AppState {
    pub lines_per_beat: u16,
    pub bpm: u16,
    pub octave: u16,
    pub is_playing: bool,
    pub selected_pattern: usize,
    pub patterns: HashMap<PatternId, Arc<Pattern>>,
    pub song: Vec<PatternId>,
    pub loop_range: Option<(usize, usize)>,
    pub instruments: Vec<Option<Instrument>>,
    pub tracks: Vec<Track>,
}

impl AppState {
    pub fn pattern(&self, idx: usize) -> Option<&Arc<Pattern>> {
        self.song.get(idx).and_then(|id| self.patterns.get(id))
    }

    pub fn next_pattern(&self, current: usize) -> usize {
        let (start, end) = match self.loop_range {
            Some(range) => range,
            None => (0, self.song.len() - 1),
        };
        let mut next = current + 1;
        if next > end {
            next = start;
        }
        next
    }

    pub fn is_track_muted(&self, track: usize) -> bool {
        self.tracks[track].muted
    }

    pub fn master_bus(&self) -> &Track {
        // the master track always exists so it's ok to unwrap here
        self.tracks.last().unwrap()
    }
}

#[derive(Clone)]
pub struct Track {
    pub id: TrackId,
    pub effects: Vec<Device>,
    pub track_type: TrackType,
    pub name: Option<String>,
    pub volume: Volume,
    pub muted: bool,
    pub rms: Arc<[AtomicF64; 2]>,
}

impl Track {
    fn new(volume: Volume, rms: Arc<[AtomicF64; 2]>) -> Self {
        Self {
            id: TrackId::new(),
            volume,
            effects: vec![],
            track_type: TrackType::Instrument,
            name: None,
            muted: false,
            rms,
        }
    }

    pub fn rms(&self) -> (f32, f32) {
        (
            self.rms[0].load(Ordering::Relaxed) as f32,
            self.rms[1].load(Ordering::Relaxed) as f32,
        )
    }
}

#[derive(Clone)]
pub struct Device {
    pub id: DeviceId,
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

    let mut app_state = AppState {
        bpm: 120,
        lines_per_beat: 4,
        octave: 4,
        is_playing: false,
        patterns: HashMap::new(),
        song: Vec::new(),
        selected_pattern: 0,
        loop_range: Some((0, 0)),
        instruments: vec![None; INSTRUMENT_TRACKS],
        tracks: Vec::new(),
    };

    let preview_track_id = TrackId::new();

    // Triple buffers are used to share app state with the engine and vice versa. This should
    // ensure that both threads always have a coherent view of the other thread's state.
    let (app_state_input, app_state_output) = TripleBuffer::new(&app_state).split();
    let (engine_state_input, engine_state_output) = TripleBuffer::new(&engine_state).split();

    // Create master track
    let master = engine::Track::default();
    let volume = Volume::new(-6.0, master.volume.clone());
    let rms = master.rms_out.clone();
    let mut track = Track::new(volume, rms);
    track.name = Some(String::from("Master"));
    track.track_type = TrackType::Bus;
    app_state.tracks.push(track);

    let (producer, consumer) = RingBuffer::<EngineCommand>::new(64).split();
    let engine = Engine::new(
        engine_state,
        engine_state_input,
        consumer,
        master,
        preview_track_id,
    );

    // We'll manage size manually so we can delete devices in the engine on eviction
    let preview_cache = LruCache::unbounded();

    let app = App {
        state: app_state,
        state_buf: app_state_input,
        producer,
        file_browser: FileBrowser::with_path("./sounds")?,
        device_params: HashMap::new(),
        preview_track_id,
        preview_cache,
    };
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
    CreateTrack(usize),
    ParamInc(DeviceId, usize, StepSize),
    ParamDec(DeviceId, usize, StepSize),
}

impl Msg {
    pub fn is_exit(&self) -> bool {
        matches!(self, Self::Exit)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct PatternId(u64);

impl Display for PatternId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone)]
pub struct Volume {
    value: Arc<AtomicF64>,
    value_db: f64,
}

impl Volume {
    fn new(db: f64, output: Arc<AtomicF64>) -> Self {
        let mut v = Self {
            value_db: 0.0,
            value: output,
        };
        v.set(db);
        v
    }

    pub fn db(&self) -> f64 {
        self.value_db
    }

    pub fn val(&self) -> f64 {
        self.value.load(Ordering::Relaxed)
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
        let new = f64::powf(10.0, db / 20.0);
        self.value.store(new, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug, Default)]
pub struct DeviceId(Ulid);

impl DeviceId {
    pub fn new() -> Self {
        Self(Ulid::new())
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug, Default)]
pub struct TrackId(Ulid);

impl TrackId {
    pub fn new() -> Self {
        Self(Ulid::new())
    }
}
