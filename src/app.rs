use crate::engine::{EngineCommand, EngineParam, EngineParams};
use crate::input;
use crate::input::{CommandState, Focus, Input, InputQueue};
use crate::param::Param;
use crate::pattern::{Editor, Move, MAX_TRACKS};
use crate::sampler::Sampler;
use crate::ui;
use crate::ui::editor::EditorState;
use anyhow::{anyhow, Result};
use camino::{Utf8Path, Utf8PathBuf};
use ringbuf::{Consumer, Producer};
use std::fs;
use std::fs::DirEntry;
use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use termion::{input::MouseTerminal, raw::IntoRawMode, screen::AlternateScreen};
use tui::{backend::TermionBackend, widgets::ListState, Terminal};

pub struct TrackSettings {
    pub sample_path: Utf8PathBuf,
    pub params: Vec<(String, Param)>,
}

pub struct App {
    cons: Consumer<AppCommand>,
    prod: Producer<EngineCommand>,

    pub editor: Editor,

    pub selected_track: usize,
    pub instruments: Vec<Option<TrackSettings>>,

    pub file_browser: FileBrowser,
    pub current_line: usize,
    pub should_stop: bool,
    pub engine_params: EngineParams,

    pub focus: Focus,
    pub files: ListState,
    pub instrument_list: ListState,
    pub params: ListState,
    pub edit_state: EditorState,
    pub command: CommandState,
}

impl App {
    pub fn new(
        params: EngineParams,
        cons: Consumer<AppCommand>,
        prod: Producer<EngineCommand>,
    ) -> Result<Self> {
        let file_browser = FileBrowser::with_path("./sounds")?;
        let mut instruments = Vec::with_capacity(MAX_TRACKS);
        for _ in 0..MAX_TRACKS {
            instruments.push(None);
        }

        let mut file_state = ListState::default();
        file_state.select(Some(0));

        Ok(App {
            cons,
            prod,
            editor: Editor::new(),
            selected_track: 0,
            current_line: 0,
            instruments,
            should_stop: false,
            engine_params: params,
            file_browser,
            params: ListState::default(),
            instrument_list: ListState::default(),
            files: file_state,
            edit_state: EditorState::new(),
            focus: Focus::Editor,
            command: CommandState {
                buffer: String::with_capacity(1024),
            },
        })
    }

    pub fn run_commands(&mut self) {
        while let Some(update) = self.cons.pop() {
            match update {
                AppCommand::SetCurrentTick(tick) => {
                    let pattern = self.editor.current_pattern();
                    self.current_line = tick % pattern.num_lines;
                }
            }
        }
    }

    pub fn run(mut self) -> Result<()> {
        let mut input = InputQueue::new();
        let stdout = io::stdout().into_raw_mode()?;
        let stdout = MouseTerminal::from(stdout);
        let stdout = AlternateScreen::from(stdout);
        let backend = TermionBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        loop {
            self.run_commands();
            if self.should_stop {
                return Ok(());
            }
            terminal.draw(|f| ui::draw(f, &mut self))?;
            match input.next()? {
                // TODO: don't exit on error from handle_input but print to console
                Input::Key(key) => input::handle(key, &mut self)?,
                Input::Tick => {}
            }
        }
    }

    pub fn take(&mut self, action: Action) -> Result<()> {
        match action {
            Action::Exit => {
                self.should_stop = true;
            }
            Action::LoadSound(i, path) => {
                let sound = Sampler::load_sound(&path)?;
                self.instruments[i] = Some(TrackSettings {
                    sample_path: path,
                    params: Vec::new(),
                });
                self.engine_send(EngineCommand::LoadSound(i, Arc::new(sound)))?;
            }
            Action::PreviewSound(path) => {
                let sound = Sampler::load_sound(&path)?;
                self.engine_send(EngineCommand::PreviewSound(Arc::new(sound)))?;
            }
            Action::InsertNote(pitch) => {
                let oct = self.engine_params.get(EngineParam::Octave) as u8;
                let pitch = oct * 12 + pitch;
                self.editor.set_pitch(pitch);
                self.engine_send(EngineCommand::InputNote(self.editor.cursor, pitch))?;
                self.take(Action::MoveCursor(Move::Down))?;
            }
            Action::InsertNumber(num) => {
                self.editor.set_number(num);
                self.engine_send(EngineCommand::InputNumber(self.editor.cursor, num))?;
            }
            Action::ChangeValue(delta) => {
                self.editor.change_value(delta);
                self.engine_send(EngineCommand::ChangeValue(self.editor.cursor, delta))?;
            }
            Action::DeleteNote => {
                self.editor.delete_value();
                self.engine_send(EngineCommand::DeleteValue(self.editor.cursor))?;
            }
            Action::TogglePlay => {
                let val = self.engine_params.is_playing.load(Ordering::Relaxed);
                self.engine_params.is_playing.store(!val, Ordering::Relaxed);
            }
            Action::IncrParam(param_index) => {
                let track = self.selected_track;
                if let Some(track) = &mut self.instruments[track] {
                    if let Some((_, param)) = track.params.get_mut(param_index) {
                        param.incr();
                    }
                }
            }
            Action::DecrParam(param_index) => {
                let track = self.selected_track;
                if let Some(track) = &mut self.instruments[track] {
                    if let Some((_, param)) = track.params.get_mut(param_index) {
                        param.decr();
                    }
                }
            }
            Action::UpdateEngineParam(param, value) => {
                let param = match param {
                    EngineParam::Bpm => &self.engine_params.bpm,
                    EngineParam::LinesPerBeat => &self.engine_params.lines_per_beat,
                    EngineParam::Octave => &self.engine_params.octave,
                };
                param.store(value.parse()?, Ordering::Relaxed);
            }
            Action::MoveCursor(cursor_move) => {
                self.editor.move_cursor(cursor_move);
                self.selected_track = self.editor.selected_track();
            }
        }
        Ok(())
    }

    fn engine_send(&mut self, cmd: EngineCommand) -> Result<()> {
        if self.prod.push(cmd).is_err() {
            Err(anyhow!("unable to send message to engine"))
        } else {
            Ok(())
        }
    }
}

pub enum AppCommand {
    SetCurrentTick(usize),
}

pub enum Action {
    Exit,
    LoadSound(usize, Utf8PathBuf),
    PreviewSound(Utf8PathBuf),
    InsertNote(u8),
    InsertNumber(i32),
    DeleteNote,
    ChangeValue(i32),
    TogglePlay,
    IncrParam(usize),
    DecrParam(usize),
    UpdateEngineParam(EngineParam, String),
    MoveCursor(Move),
}

pub struct FileBrowser {
    entries: Vec<DirEntry>,
    dir: Utf8PathBuf,
    short_dir: Utf8PathBuf,
}

impl FileBrowser {
    pub fn with_path<P: AsRef<Utf8Path>>(path: P) -> Result<FileBrowser> {
        let mut fb = FileBrowser {
            entries: Vec::new(),
            dir: Utf8PathBuf::new(),
            short_dir: Utf8PathBuf::new(),
        };
        fb.move_to(path)?;
        Ok(fb)
    }

    pub fn move_up(&mut self) -> Result<()> {
        if let Some(parent) = self.dir.clone().parent() {
            self.move_to(parent)?;
        }
        Ok(())
    }

    pub fn move_to<P: AsRef<Utf8Path>>(&mut self, path: P) -> Result<()> {
        self.entries.clear();
        for entry in fs::read_dir(path.as_ref())? {
            let entry = entry?;
            if entry.path().is_dir() || entry.path().extension().map_or(false, |ext| ext == "wav") {
                self.entries.push(entry);
            }
        }
        self.dir = Utf8PathBuf::from_path_buf(path.as_ref().canonicalize()?)
            .map_err(|path| anyhow!("invalid path {}", path.display()))?;

        self.short_dir.clear();
        let parts: Vec<_> = self.dir.components().collect();
        if parts.len() > 3 {
            for part in &parts[..parts.len() - 1] {
                self.short_dir
                    .push(part.as_str().chars().next().unwrap().to_string());
            }
            self.short_dir.push(parts.last().unwrap());
        } else {
            self.short_dir = self.dir.clone();
        }
        self.entries.sort_by_key(|e| e.path());
        Ok(())
    }

    pub fn num_entries(&self) -> usize {
        self.entries.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = String> + '_ {
        self.entries
            .iter()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
    }

    pub fn get(&self, i: usize) -> Option<Utf8PathBuf> {
        self.entries
            .get(i)
            .and_then(|entry| Utf8PathBuf::from_path_buf(entry.path()).ok())
    }

    pub fn current_dir(&self) -> String {
        self.short_dir.to_string()
    }
}
