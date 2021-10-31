use crate::input;
use crate::input::{CommandState, Focus, Input, InputQueue, Move};
use crate::pattern::{Position, MAX_COLS, NUM_TRACK_LANES};
use crate::sampler::Sampler;
use crate::state::{AppControl, SharedState};
use crate::ui;
use crate::ui::editor::EditorState;
use anyhow::{anyhow, Result};
use camino::{Utf8Path, Utf8PathBuf};
use std::fs;
use std::fs::DirEntry;
use std::io;
use termion::{input::MouseTerminal, raw::IntoRawMode, screen::AlternateScreen};
use tui::{backend::TermionBackend, widgets::ListState, Terminal};

pub struct App {
    pub control: AppControl,

    pub file_browser: FileBrowser,
    pub should_stop: bool,
    pub cursor: Position,
    pub focus: Focus,
    pub files: ListState,
    pub instrument_list: ListState,
    pub params: ListState,
    pub edit_state: EditorState,
    pub command: CommandState,
}

impl App {
    pub fn new(store: AppControl) -> Result<Self> {
        let mut file_state = ListState::default();
        file_state.select(Some(0));

        Ok(App {
            cursor: Position { line: 0, column: 0 },
            control: store,
            should_stop: false,
            file_browser: FileBrowser::with_path("./sounds")?,
            params: ListState::default(),
            instrument_list: ListState::default(),
            files: file_state,
            edit_state: EditorState::default(),
            focus: Focus::Editor,
            command: CommandState {
                buffer: String::with_capacity(1024),
            },
        })
    }

    pub fn run(mut self) -> Result<()> {
        let mut input = InputQueue::new();
        let stdout = io::stdout().into_raw_mode()?;
        let stdout = MouseTerminal::from(stdout);
        let stdout = AlternateScreen::from(stdout);
        let backend = TermionBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        loop {
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
                let sound = Sampler::load_sound(path)?;
                self.control.set_sound(i, sound);
            }
            Action::PreviewSound(path) => {
                let sound = Sampler::load_sound(path)?;
                self.control.preview_sound(sound)?
            }
            Action::InsertNote(pitch) => {
                let oct = self.control.octave() as u8;
                let pitch = oct * 12 + pitch;
                let pos = self.cursor;
                self.control.update_pattern(|p| p.set_pitch(pos, pitch));
                self.move_cursor(Move::Down);
            }
            Action::InsertNumber(num) => {
                let pos = self.cursor;
                self.control.update_pattern(|p| p.set_number(pos, num));
            }
            Action::ChangeValue(delta) => {
                let pos = self.cursor;
                self.control.update_pattern(|p| p.change_value(pos, delta));
            }
            Action::DeleteNote => {
                let pos = self.cursor;
                self.control.update_pattern(|p| p.delete_value(pos));
            }
            Action::TogglePlay => {
                self.control.toggle_play();
            }
            Action::SetBpm(bpm) => {
                self.control.set_bpm(bpm.parse()?);
            }
            Action::SetOctave(octave) => {
                self.control.set_octave(octave.parse()?);
            }
        }
        Ok(())
    }

    pub fn move_cursor(&mut self, m: Move) {
        let pattern = self.control.pattern();
        let height = pattern.num_lines;
        let cursor = &mut self.cursor;
        let step = 1;
        match m {
            Move::Left if cursor.column == 0 => {}
            Move::Left => cursor.column -= step,
            Move::Right => cursor.column = usize::min(cursor.column + step, MAX_COLS - step),
            Move::Start => cursor.column = 0,
            Move::End => cursor.column = MAX_COLS - step,
            Move::Up if cursor.line == 0 => {}
            Move::Up => cursor.line -= 1,
            Move::Down => cursor.line = usize::min(height - 1, cursor.line + 1),
            Move::Top => cursor.line = 0,
            Move::Bottom => cursor.line = height - 1,
        }
    }

    pub fn selected_track(&self) -> usize {
        self.cursor.column / NUM_TRACK_LANES
    }
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
    SetBpm(String),
    SetOctave(String),
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
