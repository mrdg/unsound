use anyhow::{anyhow, Result};
use camino::Utf8PathBuf;
use rand::prelude::*;
use ringbuf::{Producer, RingBuffer};
use triple_buffer::{Input, Output, TripleBuffer};

use crate::engine::Engine;
use crate::files::FileBrowser;
use crate::pattern::{Position, StepSize, DEFAULT_PATTERN_COUNT, MAX_PATTERNS, MAX_TRACKS};
use crate::sampler::Sampler;
use crate::view;
use crate::view::{InputQueue, View};
use crate::{engine::EngineCommand, pattern::Pattern, sampler::Sound};
use std::collections::HashMap;
use std::io;
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
}

impl App {
    fn new(
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
                    let input_buf = self.state_buf.input_buffer();
                    input_buf.clone_from(&self.state);
                    self.state_buf.publish();
                }
                view::Input::Tick => {}
            }
        }
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
                if self
                    .producer
                    .push(EngineCommand::PreviewSound(snd))
                    .is_err()
                {
                    return Err(anyhow!("unable to send message to engine"));
                }
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
        }

        Ok(())
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
            let snd = Arc::new(Sampler::load_sound(path.clone())?);
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
}

#[derive(Clone)]
pub struct EngineState {
    pub current_tick: usize,
    pub current_pattern: usize,
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
}

pub fn new() -> Result<(App, Engine)> {
    let mut sounds = Vec::with_capacity(MAX_TRACKS);
    for _ in 0..sounds.capacity() {
        sounds.push(None);
    }

    let patterns = HashMap::new();
    let song = Vec::new();

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
    };

    let engine_state = EngineState {
        current_pattern: 0,
        current_tick: 0,
    };

    let (app_input, app_output) = TripleBuffer::new(&app_state).split();
    let (engine_input, engine_output) = TripleBuffer::new(&engine_state).split();

    let (producer, consumer) = RingBuffer::<EngineCommand>::new(16).split();

    let engine = Engine::new(engine_state, engine_input, app_output, consumer);
    let mut app = App::new(app_state, app_input, engine_output, producer)?;

    for _ in 0..DEFAULT_PATTERN_COUNT {
        app.send(Msg::CreatePattern(None))?
    }

    Ok((app, engine))
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
}

impl Msg {
    pub fn is_exit(&self) -> bool {
        matches!(self, Self::Exit)
    }
}

pub trait SharedState {
    fn shared_state(&self) -> (&AppState, &EngineState);

    fn app(&self) -> &AppState {
        self.shared_state().0
    }
    fn engine(&self) -> &EngineState {
        self.shared_state().1
    }

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

    fn active_pattern_index(&self) -> usize {
        self.engine().current_pattern
    }

    fn current_line(&self) -> usize {
        // TODO: lines vs ticks
        self.engine().current_tick
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
    fn shared_state(&self) -> (&AppState, &EngineState) {
        (self.app_state, self.engine_state)
    }
}

impl<'a> ViewContext<'a> {
    pub fn patterns(&self) -> impl Iterator<Item = &Arc<Pattern>> {
        self.song()
            .iter()
            .map(move |id| self.app().patterns.get(id).unwrap())
    }
}

pub struct AudioContext<'a> {
    app_state: &'a AppState,
    engine_state: &'a EngineState,
}

impl<'a> AudioContext<'a> {
    pub fn new(app_state: &'a AppState, engine_state: &'a EngineState) -> Self {
        Self {
            app_state,
            engine_state,
        }
    }

    pub fn next_pattern(&self) -> usize {
        let (start, end) = match self.app().loop_range {
            Some(range) => range,
            None => (0, self.app().song.len() - 1),
        };
        let mut next = self.engine().current_pattern + 1;
        if next > end {
            next = start;
        }
        next
    }
}

impl<'a> SharedState for AudioContext<'a> {
    fn shared_state(&self) -> (&AppState, &EngineState) {
        (self.app_state, self.engine_state)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct PatternID(u64);

impl Display for PatternID {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
