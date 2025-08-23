use std::collections::HashMap;
use std::fmt::{self, Display, Formatter};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use atomic_float::AtomicF64;
use camino::Utf8PathBuf;
use lru::LruCache;
use ratatui::style::Color;
use ringbuf::Producer;
use triple_buffer::Input;

use crate::audio_graph::{AudioGraph, NodeId, Schedule, TrackNode};
use crate::delay::Delay;
use crate::engine::{
    Buffer, DeleteMode, EngineCommand, Event, Node, Note, Pattern as EnginePattern, Plugin,
    Track as EngineTrack, TrackParams, TICKS_PER_LINE,
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
    pub file_browser: FileBrowser,

    params: HashMap<NodeId, Arc<dyn Params>>,
    preview_cache: LruCache<Utf8PathBuf, Arc<Sound>>,
    patterns: HashMap<PatternId, Pattern>,

    pub tracks: Vec<Track>,
    pub instruments: Vec<Option<Device>>,

    pub audio_graph: AudioGraph,
}

impl App {
    pub fn new(
        app_state: AppState,
        state_input: Input<AppState>,
        mut eng_cmd_prod: Producer<EngineCommand>,
        file_browser: FileBrowser,
    ) -> Self {
        let preview_cache = LruCache::new(NonZeroUsize::new(64).unwrap());

        let audio_graph = AudioGraph::new();
        for id in &[
            audio_graph.main_output,
            audio_graph.tmp_buffer1,
            audio_graph.tmp_buffer2,
        ] {
            push_to_engine(
                &mut eng_cmd_prod,
                EngineCommand::CreateBuffer(*id, Buffer::default()),
            );
        }

        let mut app = Self {
            state: app_state,
            state_buf: state_input,
            producer: eng_cmd_prod,
            file_browser,
            params: HashMap::new(),
            preview_cache,
            engine_state: EngineState::default(),
            patterns: HashMap::new(),
            tracks: Vec::new(),
            instruments: vec![None; 16],
            audio_graph,
        };

        app.send(Msg::CreateTrack(
            0,
            Some(app.audio_graph.main_output),
            TrackType::Bus,
            Some(String::from("Master")),
        ))
        .expect("create master track");

        app
    }

    pub fn send(&mut self, msg: Msg) -> Result<()> {
        self.dispatch(msg)?;
        self.recompile_patterns();
        self.state.process_schedule = self.audio_graph.sort();
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
                let sampler: Box<dyn Plugin + Send> = Box::new(Sampler::new(snd));

                let node_id = self.audio_graph.add_instrument();
                self.params.insert(node_id, sampler.params());
                let node = Node::new(sampler);

                push_to_engine(&mut self.producer, EngineCommand::CreateNode(node_id, node));

                self.delete_instrument(idx);
                self.instruments[idx] = Some(Device::new(node_id, path.file_name().unwrap()));
            }
            LoadEffect(idx, effect) => {
                match effect.as_str() {
                    "delay" => {
                        let node_id = self.audio_graph.add_effect();
                        let delay: Box<dyn Plugin + Send> = Box::new(Delay::new(44100 / 8));
                        self.params.insert(node_id, delay.params());

                        let node = Node::new(delay);
                        push_to_engine(
                            &mut self.producer,
                            EngineCommand::CreateNode(node_id, node),
                        );

                        let track = &mut self.tracks[idx];
                        let input = track
                            .effects
                            .last()
                            .map(|d| d.node_id)
                            .unwrap_or(track.node.buffer);

                        self.audio_graph.connect(input, node_id);
                        self.audio_graph.connect(node_id, track.node.output);
                        self.tracks[idx].effects.push(Device::new(node_id, "Delay"));
                    }
                    _ => return Err(anyhow!("unknown effect {effect}")),
                };
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

                let output = self.audio_graph.main_output;
                push_to_engine(
                    &mut self.producer,
                    EngineCommand::PreviewSound(output, sound),
                );
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
            CreateTrack(idx, output_node, track_type, name) => {
                let track_node = self.audio_graph.add_track();
                self.audio_graph
                    .connect(track_node.buffer, track_node.output);

                let output_node =
                    output_node.unwrap_or_else(|| self.tracks.last().unwrap().node.buffer);
                self.audio_graph.connect(track_node.output, output_node);

                let track_output = EngineTrack::new();
                self.params.insert(track_node.output, track_output.params());

                let track = Track::new(
                    track_node.clone(),
                    track_type,
                    name,
                    track_output.rms_out.clone(),
                );
                self.tracks.insert(idx, track);

                if matches!(track_type, TrackType::Instrument) {
                    for pattern in &mut self.patterns.values_mut() {
                        pattern.add_track(idx);
                    }
                }

                let buffer = Buffer::default();
                let cmd = EngineCommand::CreateBuffer(track_node.buffer, buffer);
                push_to_engine(&mut self.producer, cmd);

                let track_output: Box<dyn Plugin + Send> = Box::new(track_output);
                let node = Node::new(track_output);
                let cmd = EngineCommand::CreateNode(track_node.output, node);
                push_to_engine(&mut self.producer, cmd);
            }
            DeleteTrack(idx) => {
                let track = self.tracks.remove(idx);
                self.params.remove(&track.node.output);
                if matches!(track.track_type, TrackType::Instrument) {
                    for pattern in &mut self.patterns.values_mut() {
                        pattern.delete_track(idx);
                    }
                }

                // Send a note off for this track to all instruments.
                for instr in &mut self.instruments {
                    let Some(instr) = instr else { continue };
                    let note_off = Event::new(Note::Off, 0, track.node.buffer, instr.node_id);
                    push_to_engine(&mut self.producer, EngineCommand::NoteEvent(note_off));
                }

                // Deleting the output node will gradually fade out the track's audio. Any other
                // nodes on the track will be cleaned up once the output node has been freed.
                let node_id = track.node.output;
                push_to_engine(
                    &mut self.producer,
                    EngineCommand::DeleteNode(node_id, DeleteMode::FadeOut),
                );
            }
            RenameTrack(idx, name) => {
                self.tracks[idx].name = name;
            }
            ParamInc(node_id, param_idx, step_size) => {
                self.params(node_id).get_param(param_idx).incr(step_size);
            }
            ParamDec(node_id, param_idx, step_size) => {
                self.params(node_id).get_param(param_idx).decr(step_size);
            }
            ParamSet(node_id, param_idx, value) => {
                self.params(node_id).get_param(param_idx).set(value);
            }
            DeleteInstrument(idx) => {
                self.delete_instrument(idx);
            }
            ToggleMute(track_idx) => {
                let id = self.tracks[track_idx].node.output;
                self.params(id).get_param(TrackParams::MUTE).toggle();
            }
            TrackVolumeIncr(track_idx) => {
                let id = self.tracks[track_idx].node.output;
                self.params(id)
                    .get_param(TrackParams::VOLUME)
                    .incr(StepSize::Large);
            }
            TrackVolumeDecr(track_idx) => {
                let idx = self.tracks[track_idx].node.output;
                self.params(idx)
                    .get_param(TrackParams::VOLUME)
                    .decr(StepSize::Large);
            }
            Command(cmd) => match cmd {
                AppCommand::DropNode(node_id, plugin) => {
                    drop(plugin);
                    self.drop_node(node_id);
                }
                AppCommand::DropBuffer(node_id, buffer) => {
                    drop(buffer);
                    self.drop_node(node_id);
                }
            },
        }

        Ok(())
    }

    fn drop_node(&mut self, node_id: NodeId) {
        self.audio_graph.remove_node(node_id);
        for node_id in self.audio_graph.orphaned_nodes() {
            if self.audio_graph.mark_deleted(node_id) {
                // The nodes in the audio graph will be freed once the engine returns the deleted nodes
                push_to_engine(&mut self.producer, EngineCommand::ForceDeleteNode(node_id));
            }
        }
    }

    pub fn params(&self, node_id: NodeId) -> &Arc<dyn Params> {
        self.params.get(&node_id).unwrap()
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

    fn delete_instrument(&mut self, idx: usize) {
        if let Some(instr) = self.instruments[idx].take() {
            self.params.remove(&instr.node_id);
            push_to_engine(
                &mut self.producer,
                EngineCommand::DeleteNode(instr.node_id, DeleteMode::FadeOut),
            );
        }
    }
}

fn push_to_engine(producer: &mut Producer<EngineCommand>, cmd: EngineCommand) {
    producer
        .push(cmd)
        .unwrap_or_else(|_| panic!("ring buffer should always have capacity"))
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
            let Some(Some(instr)) = instruments.get(instr_idx as usize) else {
                continue;
            };
            let buffer = tracks[i].node.buffer;
            let velocity = step.velocity();
            for pitch in step.notes() {
                let note = if pitch == NOTE_OFF {
                    Note::Off
                } else {
                    Note::On(pitch, velocity)
                };
                let note = Event::new(note, note_offset, buffer, instr.node_id);
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
    DropNode(NodeId, Node),
    DropBuffer(NodeId, Buffer),
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
    pub process_schedule: Schedule,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            bpm: 120,
            lines_per_beat: 4,
            octave: 4,
            is_playing: false,
            patterns: HashMap::new(),
            song: Vec::new(),
            selected_pattern: 0,
            loop_range: Some((0, 0)),
            process_schedule: Schedule::default(),
        }
    }
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
    pub node: TrackNode,
    pub effects: Vec<Device>,
    pub track_type: TrackType,
    pub name: Option<String>,
    rms: Arc<[AtomicF64; 2]>,
}

impl Track {
    fn new(
        node: TrackNode,
        track_type: TrackType,
        name: Option<String>,
        rms: Arc<[AtomicF64; 2]>,
    ) -> Self {
        Self {
            node,
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
    pub node_id: NodeId,
    pub name: String,
}

impl Device {
    fn new(node_id: NodeId, name: &str) -> Self {
        Self {
            node_id,
            name: name.to_string(),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub enum TrackType {
    Instrument,
    Bus,
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
    CreateTrack(usize, Option<NodeId>, TrackType, Option<String>),
    DeleteTrack(usize),
    RenameTrack(usize, Option<String>),
    ParamInc(NodeId, usize, StepSize),
    ParamDec(NodeId, usize, StepSize),
    ParamSet(NodeId, usize, f32),
    ToggleMute(usize),
    TrackVolumeIncr(usize),
    TrackVolumeDecr(usize),
    Command(AppCommand),
}

impl Msg {
    pub fn is_exit(&self) -> bool {
        matches!(self, Self::Exit)
    }

    pub fn is_noop(&self) -> bool {
        matches!(self, Self::Noop)
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
