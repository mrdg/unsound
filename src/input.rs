use crate::ui::ListCursorExt;
use crate::{
    app::{Action, App},
    host::HostParam,
};
use anyhow::{anyhow, Result};
use std::{
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};
use termion::{event::Key, input::TermRead};

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
    FileBrowser,
}

pub enum Cursor {
    Left,
    Right,
    Down,
    Up,
}

pub struct CommandState {
    pub buffer: String,
}

pub enum Input {
    Key(Key),
    Tick,
}

pub struct InputQueue {
    events: Receiver<Input>,
}

impl InputQueue {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        {
            let sender = sender.clone();
            thread::spawn(move || {
                let stdin = io::stdin();
                for evt in stdin.keys() {
                    if let Ok(key) = evt {
                        sender.send(Input::Key(key)).expect("send keyboard input");
                    }
                }
            })
        };
        thread::spawn(move || loop {
            if sender.send(Input::Tick).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(33));
        });
        Self { events: receiver }
    }

    pub fn next(&mut self) -> Result<Input> {
        self.events
            .recv()
            .map_err(|err| anyhow!("input receive error: {}", err))
    }
}

pub fn handle(key: Key, app: &mut App) -> Result<()> {
    match key {
        Key::Ctrl('w') => match app.focus {
            Focus::Params => {
                app.focus = Focus::Editor;
                app.params.select(None);
            }
            Focus::FileBrowser => {
                app.focus = Focus::Params;
                app.files.select(None);
                app.params.select(Some(0));
            }
            Focus::Editor => {
                app.focus = Focus::FileBrowser;
                app.files.select(Some(0));
            }
            Focus::CommandLine => {}
        },
        Key::Char(':') => {
            app.focus = Focus::CommandLine;
            return Ok(());
        }
        Key::Esc | Key::Char('\n') if app.focus == Focus::CommandLine => {
            handle_command_input(key, app)?;
            app.focus = Focus::Editor;
            return Ok(());
        }
        _ => {}
    };
    match app.focus {
        Focus::Editor => handle_editor_input(key, app)?,
        Focus::CommandLine => handle_command_input(key, app)?,
        Focus::FileBrowser => {
            let num_files = app.file_browser.iter().len();
            match key {
                Key::Char('j') => app.files.next(num_files),
                Key::Char('k') => app.files.prev(num_files),
                Key::Char('-') => {
                    let current = PathBuf::from(app.file_browser.current_dir());
                    if let Some(path) = current.parent() {
                        app.files.select(None);
                        app.file_browser.move_to(path)?;
                        app.files.select(Some(0));
                    }
                }
                Key::Char('\n') => {
                    let index = app.files.selected().unwrap();
                    if let Some(path) = app.file_browser.get(index) {
                        if path.is_dir() {
                            app.files.select(None);
                            app.file_browser.move_to(path)?;
                            app.files.select(Some(0));
                        } else {
                            app.take(Action::LoadSound(app.editor.cursor.track, path))?;
                        }
                    }
                }
                _ => {}
            }
        }
        Focus::Params => {
            let instr = &mut app.instruments[app.editor.cursor.track];
            match key {
                Key::Char('j') => app.params.next(instr.params.len()),
                Key::Char('k') => app.params.prev(instr.params.len()),
                Key::Char('J') => app.take(Action::DecrParam(
                    app.editor.cursor.track,
                    app.params.selected().unwrap(),
                ))?,
                Key::Char('K') => app.take(Action::IncrParam(
                    app.editor.cursor.track,
                    app.params.selected().unwrap(),
                ))?,
                _ => {}
            };
        }
    };
    Ok(())
}

fn handle_command_input(key: Key, app: &mut App) -> Result<()> {
    match key {
        Key::Char('\n') => exec_command(app)?,
        Key::Char(char) => app.command.buffer.push(char),
        Key::Esc => app.command.buffer.clear(),
        _ => return Err(anyhow!("invalid command input: {:?}", key)),
    };
    Ok(())
}

fn exec_command(app: &mut App) -> Result<()> {
    let parts: Vec<&str> = app.command.buffer.split(" ").collect();
    if parts.len() == 0 {
        return Err(anyhow!("invalid command"));
    }

    let action = match parts[0] {
        "quit" | "exit" => Action::Exit,
        "addtrack" => Action::AddTrack(parts[1].to_string()),
        "bpm" => Action::UpdateHostParam(HostParam::Bpm, parts[1].to_string()),
        "oct" | "octave" => Action::UpdateHostParam(HostParam::Octave, parts[1].to_string()),
        _ => return Err(anyhow!("invalid command {}", parts[0])),
    };

    app.command.buffer.clear();
    app.take(action)
}

fn handle_editor_input(key: Key, app: &mut App) -> Result<()> {
    if let Some(first_key) = app.editor.pending_key {
        match (first_key, key) {
            (Key::Char('r'), Key::Char(char)) => {
                put_note(char, app)?;
                app.editor.pending_key = None;
            }
            (_, Key::Esc) => app.editor.pending_key = None,
            _ => {}
        }
        return Ok(());
    }

    match app.mode {
        EditMode::Normal => match key {
            Key::Char(' ') => app.take(Action::TogglePlay)?,
            Key::Char('j') | Key::Down => move_cursor(Cursor::Down, app),
            Key::Char('k') | Key::Up => move_cursor(Cursor::Up, app),
            Key::Char('l') | Key::Right => move_cursor(Cursor::Right, app),
            Key::Char('h') | Key::Left => move_cursor(Cursor::Left, app),
            Key::Char('x') | Key::Backspace => delete_note(app)?,
            Key::Char('i') => app.mode = EditMode::Insert,
            Key::Char('J') => app.take(Action::ChangePitch(app.editor.cursor, -1))?,
            Key::Char('K') => app.take(Action::ChangePitch(app.editor.cursor, 1))?,
            Key::Char('r') => app.editor.pending_key = Some(key),
            _ => {}
        },
        EditMode::Insert => match key {
            Key::Char(ch) => put_note(ch, app)?,
            Key::Esc => app.mode = EditMode::Normal,
            Key::Down => move_cursor(Cursor::Down, app),
            Key::Up => move_cursor(Cursor::Up, app),
            Key::Right => move_cursor(Cursor::Right, app),
            Key::Left => move_cursor(Cursor::Left, app),
            Key::Backspace => delete_note(app)?,
            _ => {}
        },
    }
    Ok(())
}

fn put_note(key: char, app: &mut App) -> Result<()> {
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
    let pitch = app.host_params.get(HostParam::Octave) as i32 * 12 + pitch;
    let result = app.take(Action::PutNote(app.editor.cursor, pitch as i32));
    move_cursor(Cursor::Down, app);
    result
}

fn move_cursor(direction: Cursor, app: &mut App) {
    let num_lines = app.current_pattern.num_lines;
    let num_instruments = app.instruments.len();
    let c = &mut app.editor.cursor;
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

fn delete_note(app: &mut App) -> Result<()> {
    let result = app.take(Action::DeleteNote(app.editor.cursor));
    move_cursor(Cursor::Down, app);
    result
}
