use crate::host::{DeviceCommand, HostCommand, HostParam, HostParams};
use crate::input;
use crate::input::{CommandState, EditMode, Focus, Input, InputQueue};
use crate::param::Param;
use crate::sampler::{Sampler, SamplerCommand};
use crate::seq::{Event, Pattern, Slot};
use crate::ui;
use crate::ui::editor::EditorState;
use anyhow::{anyhow, Result};
use ringbuf::{Consumer, Producer};
use std::fs::DirEntry;
use std::path::PathBuf;
use std::{cmp, sync::atomic::Ordering};
use std::{fs, path::Path};
use std::{io, slice};
use termion::{input::MouseTerminal, raw::IntoRawMode, screen::AlternateScreen};
use tui::{backend::TermionBackend, widgets::ListState, Terminal};

pub struct InstrumentState {
    pub name: String,
    pub params: Vec<(String, Param)>,
}

pub struct App {
    cons: Consumer<AppCommand>,
    prod: Producer<HostCommand>,

    pub file_browser: FileBrowser,
    pub current_line: usize,
    pub current_pattern: Pattern,
    pub instruments: Vec<InstrumentState>,
    pub should_stop: bool,
    pub host_params: HostParams,

    pub mode: EditMode,
    pub focus: Focus,
    pub files: ListState,
    pub instrument_list: ListState,
    pub params: ListState,
    pub editor: EditorState,
    pub command: CommandState,
}

impl App {
    pub fn new(
        params: HostParams,
        cons: Consumer<AppCommand>,
        prod: Producer<HostCommand>,
    ) -> Result<Self> {
        let file_browser = FileBrowser::with_path("./sounds")?;

        Ok(App {
            cons,
            prod,
            current_line: 0,
            instruments: Vec::new(),
            current_pattern: Pattern::new(),
            should_stop: false,
            host_params: params,
            file_browser,
            params: ListState::default(),
            instrument_list: ListState::default(),
            files: ListState::default(),
            mode: EditMode::Normal,
            editor: EditorState::new(),
            focus: Focus::Editor,
            command: CommandState {
                buffer: String::with_capacity(1024),
            },
        })
    }

    pub fn run_commands(&mut self) {
        while let Some(update) = self.cons.pop() {
            match update {
                AppCommand::SetCurrentLine(line) => self.current_line = line,
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
            Action::AddTrack(path) => {
                let sound = Sampler::load_sound(&PathBuf::from(&path))?;
                let sampler = Sampler::with_sample(sound)?;
                let params = sampler.params();
                self.instruments.push(InstrumentState {
                    name: String::from(path),
                    params,
                });
                self.host_send(HostCommand::PutInstrument(0, Box::new(sampler)))?
            }
            Action::LoadSound(index, path) => {
                let sound = Sampler::load_sound(&path)?;
                if let Some(instr) = self.instruments.get_mut(index) {
                    instr.name = path.to_string_lossy().into_owned();
                }
                let cmd = DeviceCommand::Sampler(SamplerCommand::LoadSound(sound));
                self.host_send(HostCommand::Device(index, cmd))?;
            }
            Action::PutNote(slot, pitch) => {
                let pitch = cmp::min(cmp::max(0, pitch), 126);
                let event = Event::NoteOn { pitch };
                self.current_pattern.add_event(event, slot);
                self.host_send(HostCommand::PutPatternEvent { event, slot })?
            }
            Action::ChangePitch(slot, change) => {
                match self.current_pattern.event_at(slot.line, slot.track) {
                    Event::NoteOn { pitch } => self.take(Action::PutNote(slot, pitch + change))?,
                    _ => {}
                }
            }
            Action::DeleteNote(slot) => {
                let event = Event::Empty;
                self.current_pattern.add_event(event, slot);
                self.host_send(HostCommand::PutPatternEvent { event, slot })?
            }
            Action::TogglePlay => {
                let val = self.host_params.is_playing.load(Ordering::Relaxed);
                self.host_params.is_playing.store(!val, Ordering::Relaxed);
            }
            Action::IncrParam(device_index, param_index) => {
                if let Some(device) = self.instruments.get_mut(device_index) {
                    if let Some((_, param)) = device.params.get_mut(param_index) {
                        param.incr();
                    }
                }
            }
            Action::DecrParam(device_index, param_index) => {
                if let Some(device) = self.instruments.get_mut(device_index) {
                    if let Some((_, param)) = device.params.get_mut(param_index) {
                        param.decr();
                    }
                }
            }
            Action::UpdateHostParam(param, value) => {
                let param = match param {
                    HostParam::Bpm => &self.host_params.bpm,
                    HostParam::LinesPerBeat => &self.host_params.lines_per_beat,
                    HostParam::Octave => &self.host_params.octave,
                };
                param.store(value.parse()?, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    fn host_send(&mut self, cmd: HostCommand) -> Result<()> {
        if self.prod.push(cmd).is_err() {
            Err(anyhow!("unable to send message to host"))
        } else {
            Ok(())
        }
    }
}

pub enum AppCommand {
    SetCurrentLine(usize),
}

pub enum Action {
    Exit,
    AddTrack(String),
    LoadSound(usize, PathBuf),
    PutNote(Slot, i32),
    DeleteNote(Slot),
    ChangePitch(Slot, i32),
    TogglePlay,
    IncrParam(usize, usize),
    DecrParam(usize, usize),
    UpdateHostParam(HostParam, String),
}

pub struct FileBrowser {
    listing: Vec<DirEntry>,
    current: PathBuf,
}

impl FileBrowser {
    fn with_path(path: &str) -> Result<FileBrowser> {
        let mut listing = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            listing.push(entry);
        }
        let current = PathBuf::from(path);
        let mut fb = FileBrowser { current, listing };
        fb.listing.sort_by_key(|e| e.path());
        Ok(fb)
    }

    pub fn move_to<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let mut listing = Vec::new();
        for entry in fs::read_dir(&path)? {
            let entry = entry?;
            listing.push(entry);
        }
        self.current = path.as_ref().to_path_buf();
        self.listing = listing;
        self.listing.sort_by_key(|e| e.path());
        Ok(())
    }

    pub fn iter(&self) -> slice::Iter<'_, DirEntry> {
        self.listing.iter()
    }

    pub fn get(&self, i: usize) -> Option<PathBuf> {
        self.listing.get(i).map(|entry| entry.path())
    }

    pub fn current_dir(&self) -> String {
        match self.current.canonicalize() {
            Ok(path) => path.to_string_lossy().into_owned(),
            Err(_) => String::from(""),
        }
    }
}
