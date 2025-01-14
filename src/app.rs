use anyhow::{anyhow, Result};
use atomic_float::AtomicF64;
use camino::Utf8PathBuf;
use lru::LruCache;
use ratatui::style::Color;
use ringbuf::{Producer, RingBuffer};
use triple_buffer::{Input, Output, TripleBuffer};
use ulid::Ulid;

use crate::engine::{
    self, Engine, Plugin, Sequence, INSTRUMENT_TRACKS, PREVIEW_INSTRUMENTS_CACHE_SIZE,
};
use crate::files::FileBrowser;
use crate::params::Params;
use crate::pattern::{Step, StepSize, MAX_PATTERNS};
use crate::sampler::{self, Sampler, ROOT_PITCH};
use crate::{engine::EngineCommand, pattern::Pattern};
use std::collections::HashMap;
use std::sync::Arc;

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::ops::Range;
use std::sync::atomic::Ordering;

pub struct App {
    pub state: AppState,
    pub engine_state: EngineState,

    state_buf: Input<AppState>,
    producer: Producer<EngineCommand>,
    pub file_browser: FileBrowser,
    params: HashMap<DeviceId, Arc<dyn Params>>,
    preview_cache: LruCache<Utf8PathBuf, DeviceId>,
    preview_track_id: TrackId,
    collector: basedrop::Collector,
    patterns: HashMap<PatternId, Pattern>,
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
            SetBpm(bpm) => self.state.bpm = bpm,
            SetOct(oct) => self.state.octave = oct,
            LoadSound(idx, path) => {
                // TODO: keep settings from previous sampler?
                let snd = sampler::load_file(&path)?;
                let handle = self.collector.handle();
                let sampler: Box<dyn Plugin + Send> = Box::new(Sampler::new(snd));
                let sampler = basedrop::Owned::new(&handle, sampler);
                let sampler_id = DeviceId::new();
                self.params.insert(sampler_id, sampler.params());

                let cmd = EngineCommand::CreateInstrument(sampler_id, sampler);
                self.send_to_engine(cmd)?;

                if let Some(instr) = &self.state.instruments[idx] {
                    self.params.remove(&instr.id);
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
                if self.preview_cache.len() >= PREVIEW_INSTRUMENTS_CACHE_SIZE {
                    if let Some((_, device_id)) = self.preview_cache.pop_lru() {
                        self.send_to_engine(EngineCommand::DeleteInstrument(device_id))?;
                    }
                }
                let device_id = if let Some(id) = self.preview_cache.get(&path) {
                    *id
                } else {
                    let snd = sampler::load_file(&path)?;
                    let sampler: Box<dyn Plugin + Send> = Box::new(Sampler::new(snd));
                    let sampler_id = DeviceId::new();
                    let handle = self.collector.handle();
                    self.preview_cache.put(path.clone(), sampler_id);
                    self.send_to_engine(EngineCommand::CreateInstrument(
                        sampler_id,
                        basedrop::Owned::new(&handle, sampler),
                    ))?;
                    sampler_id
                };
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
                        self.patterns.remove(&pattern_id);
                        self.state.sequences.remove(&pattern_id);
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
            UpdatePattern(id, pattern) => {
                self.state.sequences.insert(id, pattern.compile());
                self.patterns.insert(id, pattern);
            }
            CreatePattern(idx) => {
                if self.state.sequences.len() < MAX_PATTERNS {
                    let id = self.next_pattern_id();
                    let num_instruments = self
                        .state
                        .tracks
                        .iter()
                        .filter(|track| matches!(track.track_type, TrackType::Instrument))
                        .count();

                    let pattern = Pattern::new(num_instruments);
                    self.state.sequences.insert(id, pattern.compile());
                    self.patterns.insert(id, pattern);
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
                let id = self.state.song[idx];
                let p1: &Pattern = self.patterns.get(&id).unwrap();
                let mut p2 = p1.clone();
                p2.color = random_color();
                let new_id = self.next_pattern_id();
                self.state.sequences.insert(new_id, p2.compile());
                self.patterns.insert(new_id, p2);
                self.state.song.insert(idx + 1, new_id);
            }
            ChangeDir(dir) => self.file_browser.move_to(dir)?,
            CreateTrack(idx) => {
                let track = engine::Track::new();
                let rms = track.rms_out.clone();
                let track_info = Track::new(rms);
                self.params.insert(track_info.device_id, track.params());

                let cmd = EngineCommand::CreateTrack(track_info.id, Box::new(track));
                self.send_to_engine(cmd)?;
                self.state.tracks.insert(idx, track_info);
            }
            ParamInc(device_id, param_idx, step_size) => {
                self.params(device_id).get_param(param_idx).incr(step_size);
            }
            ParamDec(device_id, param_idx, step_size) => {
                self.params(device_id).get_param(param_idx).decr(step_size);
            }
            ParamToggle(device_id, param_idx) => {
                self.params(device_id).get_param(param_idx).toggle();
            }
        }

        Ok(())
    }

    pub fn params(&self, id: DeviceId) -> &Arc<dyn Params> {
        self.params.get(&id).unwrap()
    }

    pub fn update_pattern<F>(&self, f: F) -> Msg
    where
        F: Fn(&mut Pattern),
    {
        let mut pattern = self.selected_pattern().clone();
        f(&mut pattern);

        let pattern_id = self.state.song[self.state.selected_pattern];
        Msg::UpdatePattern(pattern_id, pattern)
    }

    fn next_pattern_id(&self) -> PatternId {
        if self.state.sequences.is_empty() {
            return PatternId(0);
        }
        let mut max = 0;
        for id in self.state.sequences.keys() {
            if id.0 > max {
                max = id.0;
            }
        }
        PatternId(max + 1)
    }

    fn send_to_engine(&mut self, cmd: EngineCommand) -> Result<()> {
        if self.producer.push(cmd).is_err() {
            return Err(anyhow!("unable to send message to engine"));
        }
        Ok(())
    }

    pub fn song_iter(&self) -> impl Iterator<Item = &Pattern> {
        self.state
            .song
            .iter()
            .map(|id| self.patterns.get(id).unwrap())
    }

    pub fn selected_pattern(&self) -> &Pattern {
        let id = self.state.song[self.state.selected_pattern];
        self.patterns.get(&id).unwrap()
    }

    pub fn pattern_steps(&self, track_idx: usize, range: &Range<usize>) -> &[Step] {
        let pattern = self.selected_pattern();
        let steps = pattern.steps(track_idx);
        &steps[range.start..range.end]
    }
}

#[derive(Clone, Default)]
pub struct EngineState {
    pub current_tick: usize,
    pub current_sequence: usize,
}

impl EngineState {
    pub fn current_line(&self) -> usize {
        self.current_tick / crate::engine::TICKS_PER_LINE
    }
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
    pub sequences: HashMap<PatternId, Sequence>,
    pub song: Vec<PatternId>,
    pub loop_range: Option<(usize, usize)>,
    pub instruments: Vec<Option<Instrument>>,
    pub tracks: Vec<Track>,
}

impl AppState {
    pub fn sequence(&self, idx: usize) -> Option<&Sequence> {
        self.song.get(idx).and_then(|id| self.sequences.get(id))
    }

    pub fn next_sequence(&self, current: usize) -> usize {
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

    pub fn loop_contains(&self, idx: usize) -> bool {
        if let Some(loop_range) = self.loop_range {
            loop_range.0 <= idx && idx <= loop_range.1
        } else {
            false
        }
    }
}

#[derive(Clone)]
pub struct Track {
    pub id: TrackId,
    pub device_id: DeviceId,
    pub effects: Vec<Device>,
    pub track_type: TrackType,
    pub name: Option<String>,
    pub rms: Arc<[AtomicF64; 2]>,
}

impl Track {
    fn new(rms: Arc<[AtomicF64; 2]>) -> Self {
        Self {
            id: TrackId::new(),
            device_id: DeviceId::new(),
            effects: vec![],
            track_type: TrackType::Instrument,
            name: None,
            rms,
        }
    }

    pub fn is_bus(&self) -> bool {
        matches!(self.track_type, TrackType::Bus)
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
        current_sequence: 0,
        current_tick: 0,
    };

    let mut app_state = AppState {
        bpm: 120,
        lines_per_beat: 4,
        octave: 4,
        is_playing: false,
        sequences: HashMap::new(),
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

    let mut device_params = HashMap::new();

    // Create master track
    let master = engine::Track::new();
    let rms = master.rms_out.clone();
    let mut track = Track::new(rms);
    device_params.insert(track.device_id, master.params());

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
        params: device_params,
        preview_track_id,
        preview_cache,
        collector: basedrop::Collector::new(),
        engine_state: EngineState::default(),
        patterns: HashMap::new(),
    };
    Ok((app, app_state_output, engine, engine_state_output))
}

pub enum Msg {
    Noop,
    Exit,
    TogglePlay,
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
    UpdatePattern(PatternId, Pattern),
    ChangeDir(Utf8PathBuf),
    SetBpm(u16),
    SetOct(u16),
    CreateTrack(usize),
    ParamInc(DeviceId, usize, StepSize),
    ParamDec(DeviceId, usize, StepSize),
    ParamToggle(DeviceId, usize),
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

pub fn random_color() -> Color {
    let r = rand::random::<u8>();
    let g = rand::random::<u8>();
    let b = rand::random::<u8>();
    Color::Rgb(r, g, b)
}
