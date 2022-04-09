use anyhow::{anyhow, Result};
use camino::Utf8PathBuf;
use rand::prelude::*;
use ringbuf::Producer;
use triple_buffer::{Input, Output};

use crate::audio::Stereo;
use crate::engine::{self, INSTRUMENT_TRACKS, TOTAL_TRACKS};
use crate::files::FileBrowser;
use crate::pattern::{Position, Step, StepSize, MAX_PATTERNS};
use crate::sampler::{self, Sampler};
use crate::view;
use crate::view::{InputQueue, View};
use crate::{engine::EngineCommand, pattern::Pattern, sampler::Sound};
use std::collections::HashMap;
use std::io;
use std::ops::Range;
use std::sync::Arc;

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use termion::{input::MouseTerminal, raw::IntoRawMode, screen::AlternateScreen};
use tui::{backend::TermionBackend, Terminal};

pub struct App {
    state: AppState,
    state_buf: Input<AppState>,
    engine_state_buf: Output<EngineState>,
    producer: Producer<EngineCommand>,
    file_browser: FileBrowser,
    sound_cache: HashMap<Utf8PathBuf, Arc<Sound>>,
    id_generator: IdGenerator,
}

impl App {
    pub fn new(
        state: AppState,
        state_buf: Input<AppState>,
        engine_state_buf: Output<EngineState>,
        producer: Producer<EngineCommand>,
    ) -> Result<Self> {
        let app = Self {
            state,
            state_buf,
            engine_state_buf,
            producer,
            file_browser: FileBrowser::with_path("./sounds")?,
            sound_cache: HashMap::new(),
            id_generator: IdGenerator { current: 0 },
        };
        Ok(app)
    }

    pub fn run(&mut self) -> Result<()> {
        let mut input = InputQueue::new();
        let stdout = io::stdout().into_raw_mode()?;
        let stdout = MouseTerminal::from(stdout);
        let stdout = AlternateScreen::from(stdout);
        let backend = TermionBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Update for setup messages that were sent in main
        self.update_state();

        let mut view = View::new();
        loop {
            self.engine_state_buf.update();
            let ctx = ViewContext {
                app_state: &self.state,
                engine_state: self.engine_state_buf.output_buffer(),
                file_browser: &self.file_browser,
            };
            terminal.draw(|f| view.render(f, ctx))?;

            match input.next()? {
                view::Input::Key(key) => {
                    let msg = view.handle_input(key, ctx);
                    if msg.is_exit() {
                        return Ok(());
                    }
                    self.send(msg)?;
                    self.update_state();
                }
                view::Input::Tick => {}
            }
        }
    }

    pub fn update_state(&mut self) {
        let input_buf = self.state_buf.input_buffer();
        input_buf.clone_from(&self.state);
        self.state_buf.publish();
    }

    pub fn send(&mut self, msg: Msg) -> Result<()> {
        use Msg::*;
        match msg {
            Noop => {}
            Exit => {}
            TogglePlay => {
                self.state.is_playing = !self.state.is_playing;
            }
            SetPitch(pos, pitch) => {
                let oct = self.state.octave as u8;
                let pitch = oct * 12 + pitch;
                self.update_pattern(|p| p.set_pitch(pos, pitch));
            }
            SetSound(pos, idx) => self.update_pattern(|p| p.set_sound(pos, idx)),
            DeleteNoteValue(pos) => self.update_pattern(|p| p.delete(pos)),
            PatternInc(pos, step_size) => self.update_pattern(|p| p.inc(pos, step_size)),
            PatternDec(pos, step_size) => self.update_pattern(|p| p.dec(pos, step_size)),
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
                    let pattern = Pattern::new();
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
            SetPatternLen(len) => self.update_pattern(|p| p.length = len),
            ChangeDir(dir) => self.file_browser.move_to(dir)?,
            ToggleMute(track) => {
                let muted = &mut self.state.tracks[track].muted;
                *muted = !*muted;
            }
            VolumeInc(track) => self.track_volume(track).inc(),
            VolumeDec(track) => self.track_volume(track).dec(),
            SetVolume(track, value) => self.track_volume(track).set(value),
            CreateTrack(idx, track_type) => {
                let track_id = self.id_generator.track();

                let cmd = EngineCommand::CreateTrack(track_id, Box::new(engine::Track::new()));
                self.send_to_engine(cmd)?;

                let sampler = Box::new(Sampler::new());
                let sampler_id = self.id_generator.device();
                self.send_to_engine(EngineCommand::CreateDevice(track_id, sampler_id, sampler))?;

                let volume = Box::new(engine::Volume {});
                let volume_id = self.id_generator.device();
                self.send_to_engine(EngineCommand::CreateDevice(track_id, volume_id, volume))?;

                let track = Track {
                    id: track_id,
                    devices: vec![sampler_id, volume_id],
                    muted: false,
                    track_type,
                    volume: Volume::new(-6.0),
                };
                self.state.tracks.insert(idx, track);
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

    fn load_sound(&mut self, path: Utf8PathBuf) -> Result<Arc<Sound>> {
        let snd = if let Some(snd) = self.sound_cache.get(&path) {
            snd.clone()
        } else {
            if self.sound_cache.len() > 50 {
                // Delete random entry so the cache doesn't grow forever
                // TODO: do something smarter like LRU (maybe based on sample length)
                let key = self
                    .sound_cache
                    .keys()
                    .choose(&mut rand::thread_rng())
                    .map(|k| k.to_owned());
                if let Some(key) = key {
                    self.sound_cache.remove(&key);
                }
            }
            let snd = Arc::new(sampler::load_sound(path.clone())?);
            self.sound_cache.insert(path.clone(), snd.clone());
            snd
        };
        Ok(snd)
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

pub struct EngineState {
    pub current_tick: usize,
    pub current_pattern: usize,
    pub rms: Vec<Stereo>,
}

// TODO: use a macro to generate an allocation-free clone_from?
impl Clone for EngineState {
    fn clone(&self) -> Self {
        Self {
            current_tick: self.current_tick,
            current_pattern: self.current_pattern,
            rms: self.rms.clone(),
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.current_tick = source.current_tick;
        self.current_pattern = source.current_pattern;
        self.rms.clone_from(&source.rms);
    }
}

#[derive(Clone)]
pub struct AppState {
    lines_per_beat: u16,
    bpm: u16,
    octave: u16,
    is_playing: bool,
    selected_pattern: usize,
    patterns: HashMap<PatternID, Arc<Pattern>>,
    song: Vec<PatternID>,
    loop_range: Option<(usize, usize)>,
    sounds: Vec<Option<Arc<Sound>>>,
    tracks: Vec<Track>,
}

#[derive(Clone)]
pub struct Track {
    pub id: TrackID,
    pub devices: Vec<DeviceID>,
    pub track_type: TrackType,
    volume: Volume,
    muted: bool,
}

#[derive(Copy, Clone, Debug)]
pub enum TrackType {
    Instrument,
    #[allow(unused)]
    Bus,
    Master,
}

pub fn new() -> Result<(AppState, EngineState)> {
    let mut sounds = Vec::with_capacity(INSTRUMENT_TRACKS);
    for _ in 0..sounds.capacity() {
        sounds.push(None);
    }

    let tracks = Vec::new();
    let patterns = HashMap::new();
    let song = Vec::new();

    let engine_state = EngineState {
        current_pattern: 0,
        current_tick: 0,
        rms: vec![Stereo::ZERO; TOTAL_TRACKS],
    };

    let app_state = AppState {
        bpm: 120,
        lines_per_beat: 4,
        octave: 4,
        is_playing: false,
        patterns,
        song,
        selected_pattern: 0,
        loop_range: Some((0, 0)),
        sounds,
        tracks,
    };

    Ok((app_state, engine_state))
}

pub enum Msg {
    Noop,
    Exit,
    TogglePlay,
    SetPitch(Position, u8),
    PatternInc(Position, StepSize),
    PatternDec(Position, StepSize),
    SetSound(Position, i32),
    DeleteNoteValue(Position),
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

    fn octave(&self) -> u16 {
        self.app().octave
    }

    fn is_playing(&self) -> bool {
        self.app().is_playing
    }

    fn sounds(&self) -> &Vec<Option<Arc<Sound>>> {
        &self.app().sounds
    }

    fn pattern(&self, idx: usize) -> Option<&Arc<Pattern>> {
        self.app()
            .song
            .get(idx)
            .and_then(|id| self.app().patterns.get(id))
    }

    fn selected_pattern(&self) -> &Pattern {
        let id = self.app().song[self.app().selected_pattern];
        self.app().patterns.get(&id).unwrap()
    }

    fn selected_pattern_index(&self) -> usize {
        self.app().selected_pattern
    }

    fn song(&self) -> &Vec<PatternID> {
        &self.app().song
    }

    fn loop_contains(&self, idx: usize) -> bool {
        if let Some(loop_range) = self.app().loop_range {
            loop_range.0 <= idx && idx <= loop_range.1
        } else {
            false
        }
    }
}

#[derive(Copy, Clone)]
pub struct ViewContext<'a> {
    app_state: &'a AppState,
    engine_state: &'a EngineState,
    pub file_browser: &'a FileBrowser,
}

impl<'a> SharedState for ViewContext<'a> {
    fn app(&self) -> &AppState {
        self.app_state
    }
}

impl<'a> ViewContext<'a> {
    pub fn patterns(&self) -> impl Iterator<Item = &Arc<Pattern>> {
        self.song()
            .iter()
            .map(move |id| self.app().patterns.get(id).unwrap())
    }

    pub fn current_line(&self) -> usize {
        // TODO: lines vs ticks
        self.engine_state.current_tick
    }

    pub fn active_pattern_index(&self) -> usize {
        self.engine_state.current_pattern
    }

    pub fn rms(&self, track: usize) -> (f32, f32) {
        let rms = self.engine_state.rms[track];
        (rms.channel(0), rms.channel(1))
    }

    pub fn iter_tracks(&self) -> impl Iterator<Item = TrackView> {
        let pattern = self.selected_pattern();
        self.app_state
            .tracks
            .iter()
            .enumerate()
            .map(move |(i, track)| {
                let steps = match track.track_type {
                    TrackType::Instrument => Some(&pattern.tracks[i].steps[..pattern.length]),
                    _ => None,
                };

                TrackView {
                    steps,
                    index: i,
                    muted: track.muted,
                    volume: track.volume.db(),
                    is_master: matches!(track.track_type, TrackType::Master),
                }
            })
    }
}

pub struct TrackView<'a> {
    pub steps: Option<&'a [Step]>,
    pub index: usize,
    pub muted: bool,
    pub volume: f64,
    pub is_master: bool,
}

impl TrackView<'_> {
    pub fn steps(&self, range: &Range<usize>) -> &[Step] {
        self.steps
            .map_or(&[], |steps| &steps[range.start..range.end])
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

    pub fn tracks(&self) -> &Vec<Track> {
        &self.app_state.tracks
    }

    pub fn instrument_tracks(&self) -> impl Iterator<Item = &Track> {
        self.app_state
            .tracks
            .iter()
            .filter(|t| matches!(t.track_type, TrackType::Instrument))
    }

    pub fn master_track(&self) -> &Track {
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

    pub fn track_volume(&self, track: usize) -> f64 {
        self.app_state.tracks[track].volume.val()
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
