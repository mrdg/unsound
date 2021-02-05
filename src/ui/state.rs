use crate::app::{Action, ClientState, HostParam, ParamUpdate};
use crate::param::ParamKey;
use crate::ui::editor::EditorState;
use crate::ui::ListCursorExt;
use anyhow::{anyhow, Result};
use termion::event::Key;
use tui::widgets::ListState;

#[derive(Copy, Clone, PartialEq)]
pub enum EditMode {
    Normal,
    Insert,
}

#[derive(PartialEq)]
pub enum Focus {
    Editor,
    CommandLine,
    Params,
}

pub enum Cursor {
    Left,
    Right,
    Down,
    Up,
}

pub struct ViewState {
    pub app: ClientState,
    pub mode: EditMode,
    pub params: ListState,
    pub instruments: ListState,
    pub editor: EditorState,
    pub command: CommandState,
    pub focus: Focus,
}

pub struct CommandState {
    pub buffer: String,
}

impl ViewState {
    pub fn new(client_state: ClientState) -> Self {
        Self {
            app: client_state,
            params: ListState::default(),
            instruments: ListState::default(),
            mode: EditMode::Normal,
            editor: EditorState::new(),
            focus: Focus::Editor,
            command: CommandState {
                buffer: String::with_capacity(1024),
            },
        }
    }

    pub fn handle_input(&mut self, key: Key) -> Result<()> {
        match key {
            Key::Ctrl('w') => match self.focus {
                Focus::Params => {
                    self.focus = Focus::Editor;
                    self.params.select(None);
                }
                Focus::Editor => {
                    self.focus = Focus::Params;
                    self.params.select(Some(0));
                }
                Focus::CommandLine => {}
            },
            Key::Char(':') => {
                self.focus = Focus::CommandLine;
                return Ok(());
            }
            Key::Esc | Key::Char('\n') if self.focus == Focus::CommandLine => {
                self.handle_command_input(key)?;
                self.focus = Focus::Editor;
                return Ok(());
            }
            _ => {}
        };
        match self.focus {
            Focus::Editor => self.handle_editor_input(key)?,
            Focus::CommandLine => self.handle_command_input(key)?,
            Focus::Params => {
                let instrument = &self.app.instruments[0];
                match key {
                    Key::Char('j') => self.params.next(instrument.params.len()),
                    Key::Char('k') => self.params.prev(instrument.params.len()),
                    Key::Char('J') => {
                        let (key, _) = instrument.params[self.params.selected().unwrap()];
                        self.app.take(Action::UpdateParam(
                            Some(self.editor.cursor.track),
                            key,
                            ParamUpdate::Dec,
                        ))?;
                    }
                    Key::Char('K') => {
                        let (key, _) = instrument.params[self.params.selected().unwrap()];
                        self.app.take(Action::UpdateParam(
                            Some(self.editor.cursor.track),
                            key,
                            ParamUpdate::Inc,
                        ))?;
                    }
                    _ => {}
                };
            }
        };
        Ok(())
    }

    fn handle_command_input(&mut self, key: Key) -> Result<()> {
        match key {
            Key::Char('\n') => self.exec_command()?,
            Key::Char(char) => self.command.buffer.push(char),
            Key::Esc => self.command.buffer.clear(),
            _ => return Err(anyhow!("invalid command input: {:?}", key)),
        };
        Ok(())
    }

    fn exec_command(&mut self) -> Result<()> {
        let parts: Vec<&str> = self.command.buffer.split(" ").collect();
        if parts.len() == 0 {
            return Err(anyhow!("invalid command"));
        }

        let action = match parts[0] {
            "quit" | "exit" => Action::Exit,
            "addtrack" => Action::AddTrack(parts[1].to_string()),
            "bpm" => Action::UpdateParam(
                None,
                ParamKey::Host(HostParam::Bpm),
                ParamUpdate::Set(parts[1].to_string()),
            ),
            "oct" | "octave" => Action::UpdateParam(
                None,
                ParamKey::Host(HostParam::Octave),
                ParamUpdate::Set(parts[1].to_string()),
            ),
            _ => return Err(anyhow!("invalid command {}", parts[0])),
        };

        self.command.buffer.clear();
        self.app.take(action)
    }

    fn handle_editor_input(&mut self, key: Key) -> Result<()> {
        if let Some(first_key) = self.editor.pending_key {
            match (first_key, key) {
                (Key::Char('r'), Key::Char(char)) => {
                    self.put_note(char)?;
                    self.editor.pending_key = None;
                }
                (_, Key::Esc) => self.editor.pending_key = None,
                _ => {}
            }
            return Ok(());
        }

        match self.mode {
            EditMode::Normal => match key {
                Key::Char(' ') => self.app.take(Action::TogglePlay)?,
                Key::Char('j') | Key::Down => self.move_cursor(Cursor::Down),
                Key::Char('k') | Key::Up => self.move_cursor(Cursor::Up),
                Key::Char('l') | Key::Right => self.move_cursor(Cursor::Right),
                Key::Char('h') | Key::Left => self.move_cursor(Cursor::Left),
                Key::Char('x') | Key::Backspace => self.delete_note()?,
                Key::Char('i') => self.mode = EditMode::Insert,
                Key::Char('J') => self.app.take(Action::ChangePitch(self.editor.cursor, -1))?,
                Key::Char('K') => self.app.take(Action::ChangePitch(self.editor.cursor, 1))?,
                Key::Char('r') => self.editor.pending_key = Some(key),
                _ => {}
            },
            EditMode::Insert => match key {
                Key::Char(ch) => self.put_note(ch)?,
                Key::Esc => self.mode = EditMode::Normal,
                Key::Down => self.move_cursor(Cursor::Down),
                Key::Up => self.move_cursor(Cursor::Up),
                Key::Right => self.move_cursor(Cursor::Right),
                Key::Left => self.move_cursor(Cursor::Left),
                Key::Backspace => self.delete_note()?,
                _ => {}
            },
        }
        Ok(())
    }

    fn put_note(&mut self, key: char) -> Result<()> {
        let pitch = match key {
            'z' => 0,
            's' => 1,
            'x' => 2,
            'd' => 3,
            'c' => 4,
            'v' => 5,
            'g' => 6,
            'b' => 7,
            'h' => 8,
            'n' => 9,
            'j' => 10,
            'm' => 11,
            _ => return Ok(()),
        };
        let pitch = self.app.host_param(HostParam::Octave) as i32 * 12 + pitch;
        let result = self
            .app
            .take(Action::PutNote(self.editor.cursor, pitch as i32));
        self.move_cursor(Cursor::Down);
        result
    }

    fn move_cursor(&mut self, direction: Cursor) {
        let num_lines = self.app.current_pattern.num_lines;
        let num_instruments = self.app.instruments.len();
        let c = &mut self.editor.cursor;
        match direction {
            Cursor::Up if c.line == 0 => c.line = num_lines - 1,
            Cursor::Up => c.line -= 1,
            Cursor::Down if c.line == num_lines - 1 => c.line = 0,
            Cursor::Down => c.line += 1,
            Cursor::Right if c.track == num_instruments - 1 => c.track = 0,
            Cursor::Right => c.track += 1,
            Cursor::Left if c.track == 0 => c.track = num_instruments - 1,
            Cursor::Left => c.track -= 1,
        }
    }

    fn delete_note(&mut self) -> Result<()> {
        let result = self.app.take(Action::DeleteNote(self.editor.cursor));
        self.move_cursor(Cursor::Down);
        result
    }
}
