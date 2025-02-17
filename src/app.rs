use std::collections::HashMap;
use std::fmt::{self, Display, Formatter};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use atomic_float::AtomicF64;
use bit_set::BitSet;
use camino::Utf8PathBuf;
use lru::LruCache;
use ratatui::style::Color;
use ringbuf::{Consumer, Producer, RingBuffer};
use triple_buffer::{Input, Output, TripleBuffer};

use crate::delay::Delay;
use crate::engine::{
    Engine, EngineCommand, Event, Note, Pattern as EnginePattern, Plugin, Track as EngineTrack,
    TrackParams, MAX_INSTRUMENTS, MAX_NODES, MAX_TRACKS, SCRATCH_BUFFER, TICKS_PER_LINE,
};
use crate::files::FileBrowser;
use crate::params::Params;
use crate::pattern::{Pattern, Step, StepSize, NOTE_OFF};
use crate::sampler::{self, Sampler, Sound};

const MAX_PATTERNS: usize = 999;

pub struct App {
    pub state: AppState,
    pub engine_state: EngineState,

    state_buf: Input<AppState>,
    producer: Producer<EngineCommand>,
    consumer: Consumer<AppCommand>,
    pub file_browser: FileBrowser,

    params: HashMap<usize, Arc<dyn Params>>,
    preview_cache: LruCache<Utf8PathBuf, Arc<Sound>>,
    patterns: HashMap<PatternId, Pattern>,

    pub tracks: Vec<Track>,
    pub instruments: Vec<Option<Device>>,

    node_indices: BitSet,
}

impl App {
    pub fn send(&mut self, msg: Msg) -> Result<()> {
        while let Some(cmd) = self.consumer.pop() {
            match cmd {
                AppCommand::DropPlugin(node_index, plugin) => {
                    drop(plugin);
                    self.node_indices.insert(node_index);
                }
            }
        }

        self.dispatch(msg)?;
        self.recompile_patterns();
        let input_buf = self.state_buf.input_buffer();
        input_buf.clone_from(&self.state);
        self.state_buf.publish();

        Ok(())
    }

    fn get_node_index(&mut self, range: Range<usize>) -> Result<usize> {
        for n in range {
            if !self.node_indices.contains(n) {
                self.node_indices.insert(n);
                return Ok(n);
            }
        }
        Err(anyhow!("reached max. number of nodes"))
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
                let sampler: Box<dyn Plugin + Send> = Box::new(Sampler::new(snd));
                let sampler_index = self.get_node_index(MAX_TRACKS..MAX_NODES)?;
                self.params.insert(sampler_index, sampler.params());
                let cmd = EngineCommand::CreateNode(sampler_index, sampler);
                self.send_to_engine(cmd)?;

                if let Some(instr) = &self.instruments[idx] {
                    self.params.remove(&instr.node_index);
                    self.send_to_engine(EngineCommand::DeleteNode(instr.node_index))?;
                }

                self.instruments[idx] = Some(Device {
                    node_index: sampler_index,
                    name: path.file_name().unwrap().to_string(),
                });
                self.update_node_order();
            }
            LoadEffect(idx, effect) => {
                match effect.as_str() {
                    "delay" => {
                        let delay_index = self.get_node_index(MAX_TRACKS..MAX_NODES)?;
                        let delay: Box<dyn Plugin + Send> = Box::new(Delay::new(44100 / 8));
                        let cmd = EngineCommand::CreateNode(delay_index, delay);
                        self.send_to_engine(cmd)?;
                        self.tracks[idx].effects.push(Device {
                            node_index: delay_index,
                            name: String::from("Delay"),
                        });
                    }
                    _ => return Err(anyhow!("unknown effect {effect}")),
                };
                self.update_node_order();
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
                let sound = match self.preview_cache.get(&path) {
                    Some(sound) => sound.clone(),
                    None => {
                        let sound = Arc::new(sampler::load_file(&path)?);
                        self.preview_cache.put(path.clone(), sound.clone());
                        sound
                    }
                };

                self.send_to_engine(EngineCommand::PreviewSound(sound))?;
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
            UpdatePattern(id, pattern) => {
                self.patterns.insert(id, pattern);
            }
            CreatePattern(idx) => {
                if self.state.patterns.len() < MAX_PATTERNS {
                    let id = self.next_pattern_id();
                    let num_instruments = self
                        .tracks
                        .iter()
                        .filter(|track| matches!(track.track_type, TrackType::Instrument))
                        .count();

                    let pattern = Pattern::new(num_instruments);
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
                self.patterns.insert(new_id, p2);
                self.state.song.insert(idx + 1, new_id);
            }
            ChangeDir(dir) => self.file_browser.move_to(dir)?,
            CreateTrack(idx, output_index, track_type, name) => {
                let node_index = self.get_node_index(0..MAX_TRACKS)?;
                let engine_track = EngineTrack::new();
                let track = Track::new(
                    node_index,
                    output_index,
                    track_type,
                    name,
                    engine_track.rms_out.clone(),
                );
                self.params.insert(node_index, engine_track.params());

                if idx > self.tracks.len() {
                    self.tracks.push(track);
                } else {
                    self.tracks.insert(idx, track)
                }

                if matches!(track_type, TrackType::Instrument) {
                    for pattern in &mut self.patterns.values_mut() {
                        pattern.add_track(idx);
                    }
                }

                let engine_track: Box<dyn Plugin + Send> = Box::new(engine_track);
                let cmd = EngineCommand::CreateNode(node_index, engine_track);
                self.send_to_engine(cmd)?;
                self.update_node_order();
            }
            DeleteTrack(idx) => {
                self.tracks.remove(idx);
                for pattern in &mut self.patterns.values_mut() {
                    pattern.delete_track(idx);
                }
                self.update_node_order();
            }
            RenameTrack(idx, name) => {
                self.tracks[idx].name = name;
            }
            ParamInc(node_index, param_idx, step_size) => {
                self.params(node_index).get_param(param_idx).incr(step_size);
            }
            ParamDec(node_index, param_idx, step_size) => {
                self.params(node_index).get_param(param_idx).decr(step_size);
            }
            DeleteInstrument(idx) => {
                if let Some(instr) = &self.instruments[idx] {
                    self.params.remove(&instr.node_index);
                    self.send_to_engine(EngineCommand::DeleteNode(instr.node_index))?;
                }
                self.instruments[idx] = None;
            }
            ToggleMute(track_idx) => {
                let idx = self.tracks[track_idx].node_index;
                self.params(idx).get_param(TrackParams::MUTE).toggle();
            }
            TrackVolumeIncr(track_idx) => {
                let idx = self.tracks[track_idx].node_index;
                self.params(idx)
                    .get_param(TrackParams::VOLUME)
                    .incr(StepSize::Large);
            }
            TrackVolumeDecr(track_idx) => {
                let idx = self.tracks[track_idx].node_index;
                self.params(idx)
                    .get_param(TrackParams::VOLUME)
                    .decr(StepSize::Large);
            }
        }

        Ok(())
    }

    pub fn params(&self, node_index: usize) -> &Arc<dyn Params> {
        self.params.get(&node_index).unwrap()
    }

    pub fn update_pattern<F>(&self, mut f: F) -> Msg
    where
        F: FnMut(&mut Pattern),
    {
        let mut pattern = self.selected_pattern().clone();
        f(&mut pattern);

        let pattern_id = self.state.song[self.state.selected_pattern];
        Msg::UpdatePattern(pattern_id, pattern)
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

    fn send_to_engine(&mut self, cmd: EngineCommand) -> Result<()> {
        if self.producer.push(cmd).is_err() {
            return Err(anyhow!("unable to send message to engine"));
        }
        Ok(())
    }

    fn recompile_patterns(&mut self) {
        for (id, pattern) in &mut self.patterns {
            self.state.patterns.insert(
                *id,
                compile_pattern(&self.tracks, &self.instruments, pattern),
            );
        }
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

    fn update_node_order(&mut self) {
        let mut entries = Vec::new();

        for instr in &self.instruments {
            let Some(instr) = instr else { continue };
            entries.push(NodeEntry::new(instr.node_index, None));
        }

        for track in &self.tracks {
            let mut input = track.node_index;
            let mut output = SCRATCH_BUFFER;

            for effect in &track.effects {
                let entry = NodeEntry::new(effect.node_index, Some((input, output)));
                entries.push(entry);
                (input, output) = (output, input);
            }

            let entry = NodeEntry::new(track.node_index, Some((input, track.output_node_index)));
            entries.push(entry);
        }

        self.state.node_order = entries;
    }
}

fn compile_pattern(
    tracks: &[Track],
    instruments: &[Option<Device>],
    pattern: &Pattern,
) -> EnginePattern {
    let mut events = Vec::new();
    for (i, track) in pattern.tracks.iter().enumerate() {
        let mut pattern_offset = 0;
        for step in &track.steps {
            let offset = u8::min(TICKS_PER_LINE as u8 - 1, step.offset().unwrap_or(0));
            let note_offset = pattern_offset + offset as usize;
            pattern_offset += TICKS_PER_LINE;
            let instr_idx = step.instrument().unwrap_or(i as u8);
            let Some(instr) = &instruments[instr_idx as usize] else {
                continue;
            };
            let track_idx = tracks[i].node_index;
            let velocity = step.velocity();
            for pitch in step.notes() {
                let note = if pitch == NOTE_OFF {
                    Note::Off
                } else {
                    Note::On(pitch, velocity)
                };
                let note = Event::new(note, note_offset, track_idx, instr.node_index);
                events.push(note);
            }
        }
    }
    events.sort_by(|a, b| a.offset.cmp(&b.offset));
    EnginePattern {
        length: pattern.len() * TICKS_PER_LINE,
        events,
    }
}

#[derive(Clone, Default)]
pub struct EngineState {
    pub current_tick: usize,
    pub current_pattern: usize,
}

impl EngineState {
    pub fn current_line(&self) -> usize {
        self.current_tick / crate::engine::TICKS_PER_LINE
    }
}

pub enum AppCommand {
    DropPlugin(usize, Box<dyn Plugin + Send>),
}

#[derive(Clone)]
pub struct AppState {
    pub lines_per_beat: u16,
    pub bpm: u16,
    pub octave: u16,
    pub is_playing: bool,
    pub selected_pattern: usize,
    pub patterns: HashMap<PatternId, EnginePattern>,
    pub song: Vec<PatternId>,
    pub loop_range: Option<(usize, usize)>,
    pub node_order: Vec<NodeEntry>,
}

impl AppState {
    pub fn pattern(&self, idx: usize) -> Option<&EnginePattern> {
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
    pub node_index: usize,
    pub output_node_index: usize,
    pub effects: Vec<Device>,
    pub track_type: TrackType,
    pub name: Option<String>,
    rms: Arc<[AtomicF64; 2]>,
}

impl Track {
    fn new(
        node_index: usize,
        output_node_index: usize,
        track_type: TrackType,
        name: Option<String>,
        rms: Arc<[AtomicF64; 2]>,
    ) -> Self {
        Self {
            node_index,
            output_node_index,
            effects: vec![],
            track_type,
            name,
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
    pub node_index: usize,
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
        node_order: Vec::new(),
    };

    // Triple buffers are used to share app state with the engine and vice versa. This should
    // ensure that both threads always have a coherent view of the other thread's state.
    let (app_state_input, app_state_output) = TripleBuffer::new(&app_state).split();
    let (engine_state_input, engine_state_output) = TripleBuffer::new(&engine_state).split();

    let params = HashMap::new();
    let node_indices = BitSet::with_capacity(MAX_NODES);

    let (eng_producer, eng_consumer) = RingBuffer::<EngineCommand>::new(64).split();
    let (app_producer, app_consumer) = RingBuffer::<AppCommand>::new(64).split();

    let engine = Engine::new(engine_state, engine_state_input, eng_consumer, app_producer);

    let preview_cache = LruCache::new(NonZeroUsize::new(64).unwrap());

    let app = App {
        state: app_state,
        state_buf: app_state_input,
        producer: eng_producer,
        consumer: app_consumer,
        file_browser: FileBrowser::with_path("./sounds")?,
        params,
        preview_cache,
        engine_state: EngineState::default(),
        patterns: HashMap::new(),
        node_indices,
        tracks: Vec::new(),
        instruments: vec![None; MAX_INSTRUMENTS],
    };

    Ok((app, app_state_output, engine, engine_state_output))
}

pub enum Msg {
    Noop,
    Exit,
    TogglePlay,
    LoadSound(usize, Utf8PathBuf),
    LoadEffect(usize, String),
    DeleteInstrument(usize),
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
    CreateTrack(usize, usize, TrackType, Option<String>),
    DeleteTrack(usize),
    RenameTrack(usize, Option<String>),
    ParamInc(usize, usize, StepSize),
    ParamDec(usize, usize, StepSize),
    ToggleMute(usize),
    TrackVolumeIncr(usize),
    TrackVolumeDecr(usize),
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

pub fn random_color() -> Color {
    let r = rand::random::<u8>();
    let g = rand::random::<u8>();
    let b = rand::random::<u8>();
    Color::Rgb(r, g, b)
}

#[derive(Clone)]
pub struct NodeEntry {
    pub node_index: usize,
    pub buffers: Option<(usize, usize)>,
}

impl NodeEntry {
    fn new(node_index: usize, buffers: Option<(usize, usize)>) -> Self {
        Self {
            node_index,
            buffers,
        }
    }
}
