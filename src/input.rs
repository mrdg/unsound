use crate::app::{Action, App};
use crate::pattern::NUM_TRACK_LANES;
use crate::ui::ListCursorExt;
use anyhow::{anyhow, Result};
use std::{
    io,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};
use termion::{event::Key, input::TermRead};

#[derive(PartialEq)]
pub enum Focus {
    Editor,
    CommandLine,
    FileBrowser,
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
                for key in stdin.keys().flatten() {
                    sender.send(Input::Key(key)).expect("send keyboard input");
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
            Focus::FileBrowser => {
                app.focus = Focus::Editor;
            }
            Focus::Editor => {
                app.focus = Focus::FileBrowser;
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
            let num_files = app.file_browser.num_entries();
            match key {
                Key::Down | Key::Ctrl('n') => app.files.next(num_files),
                Key::Up | Key::Ctrl('p') => app.files.prev(num_files),
                Key::Char('[') => {
                    app.files.select(None);
                    app.file_browser.move_up()?;
                    app.files.select(Some(0));
                }
                Key::Char(' ') => {
                    let index = app.files.selected().unwrap();
                    if let Some(path) = app.file_browser.get(index) {
                        if !path.is_dir() {
                            app.take(Action::PreviewSound(path))?;
                        }
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
                            let i = app.selected_track();
                            app.take(Action::LoadSound(i, path))?;
                        }
                    }
                }
                _ => {}
            }
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
    let parts: Vec<&str> = app.command.buffer.split(' ').collect();
    if parts.is_empty() {
        return Err(anyhow!("invalid command"));
    }

    let action = match parts[0] {
        "quit" | "exit" => Action::Exit,
        "bpm" => Action::SetBpm(parts[1].to_string()),
        "oct" | "octave" => Action::SetOctave(parts[1].to_string()),
        _ => return Err(anyhow!("invalid command {}", parts[0])),
    };

    app.command.buffer.clear();
    app.take(action)
}

pub enum Move {
    Left,
    Right,
    Up,
    Down,
    Start,
    End,
    Top,
    Bottom,
}

fn handle_editor_input(key: Key, app: &mut App) -> Result<()> {
    match key {
        Key::Char(' ') => app.take(Action::TogglePlay)?,
        Key::Ctrl('n') | Key::Down => app.move_cursor(Move::Down),
        Key::Ctrl('p') | Key::Up => app.move_cursor(Move::Up),
        Key::Ctrl('f') | Key::Right => app.move_cursor(Move::Right),
        Key::Ctrl('b') | Key::Left => app.move_cursor(Move::Left),
        Key::Ctrl('a') => app.move_cursor(Move::Start),
        Key::Ctrl('e') => app.move_cursor(Move::End),
        Key::Backspace => delete_note(app)?,
        Key::Char('\n') => app.move_cursor(Move::Down),
        Key::Char(']') => app.take(Action::ChangeValue(-1))?,
        Key::Char('[') => app.take(Action::ChangeValue(1))?,
        Key::Char('}') => app.take(Action::ChangeValue(-12))?,
        Key::Char('{') => app.take(Action::ChangeValue(12))?,
        Key::Char(key) => match app.cursor.column % NUM_TRACK_LANES {
            0 => set_pitch(app, key)?,
            1 => set_sound(app, key)?,
            _ => {}
        },
        _ => {}
    };
    Ok(())
}

fn set_pitch(app: &mut App, key: char) -> Result<()> {
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
    app.take(Action::SetPitch(pitch as u8))?;
    Ok(())
}

fn set_sound(app: &mut App, key: char) -> Result<()> {
    if let Some(num) = key.to_digit(10) {
        app.take(Action::SetSound(num as u8))?;
    }
    Ok(())
}

fn delete_note(app: &mut App) -> Result<()> {
    let result = app.take(Action::DeleteNote);
    app.move_cursor(Move::Down);
    result
}
