use anyhow::{anyhow, Result};
use camino::Utf8PathBuf;
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    widgets::ListState,
};

use crate::app::{App, Msg};
use crate::engine::TrackParams;
use crate::pattern::{Selection, StepSize, INPUTS_PER_STEP};
use crate::sampler;
use crate::view::{Focus, ProjectTreeState, View};

pub fn handle_key_event(app: &App, view: &mut View, key: KeyEvent) -> Msg {
    match handle_key(app, view, key) {
        Ok(msg) => msg,
        Err(err) => {
            eprintln!("error: {}", err);
            Msg::Noop
        }
    }
}

fn handle_key(app: &App, view: &mut View, key: KeyEvent) -> Result<Msg> {
    use Msg::*;

    if key.code == KeyCode::Char('w') && key.modifiers.contains(KeyModifiers::CONTROL) {
        use Focus::*;
        view.focus = match view.focus {
            Patterns => Editor,
            Editor => ProjectTree,
            ProjectTree => FileLoader,
            FileLoader => Patterns,
            CommandLine => CommandLine,
        };
        return Ok(Noop);
    }

    if key.code == KeyCode::Char(':') && view.focus != Focus::CommandLine {
        view.focus = Focus::CommandLine;
        return Ok(Noop);
    }

    match view.focus {
        Focus::Editor => return handle_editor_input(app, view, key),
        Focus::CommandLine => return handle_command_line_input(app, view, key),
        Focus::Patterns => match key.code {
            KeyCode::Backspace => return Ok(DeletePattern(view.patterns.selected().unwrap())),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(CreatePattern(Some(view.patterns.selected().unwrap())))
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(RepeatPattern(view.patterns.selected().unwrap()))
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(ClonePattern(view.patterns.selected().unwrap()))
            }
            KeyCode::Char('l') => return Ok(LoopToggle(view.patterns.selected().unwrap())),
            KeyCode::Char('L') => return Ok(LoopAdd(view.patterns.selected().unwrap())),
            KeyCode::Enter => return Ok(SelectPattern(view.patterns.selected().unwrap())),
            _ => handle_list_input(&mut view.patterns, key),
        },
        Focus::ProjectTree => return handle_project_tree_input(app, view, key),
        Focus::FileLoader => {
            match key.code {
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    view.instruments.select_previous()
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    view.instruments.select_next()
                }
                KeyCode::Char('u') => {
                    if let Some(dir) = app.file_browser.dir.parent() {
                        view.files = ListState::default().with_selected(Some(0));
                        return Ok(ChangeDir(dir.to_path_buf()));
                    }
                }
                KeyCode::Char(' ') => {
                    let entry = &app.file_browser.entries[view.files.selected().unwrap()];
                    if sampler::can_load_file(&entry.path) {
                        return Ok(PreviewSound(entry.path.to_path_buf()));
                    }
                }
                KeyCode::Enter => {
                    let entry = &app.file_browser.entries[view.files.selected().unwrap()];
                    let msg = if entry.file_type.is_dir() {
                        view.files = ListState::default().with_selected(Some(0));
                        ChangeDir(entry.path.to_path_buf())
                    } else if sampler::can_load_file(&entry.path) {
                        LoadSound(
                            view.instruments.selected().unwrap(),
                            entry.path.to_path_buf(),
                        )
                    } else {
                        Noop
                    };
                    return Ok(msg);
                }
                _ => handle_list_input(&mut view.files, key),
            };
        }
    }

    Ok(Noop)
}

fn handle_editor_input(app: &App, view: &mut View, key: KeyEvent) -> Result<Msg> {
    use Msg::*;
    if let Some(s) = &mut view.selection {
        match key.code {
            KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                view.clipboard = Some((app.selected_pattern().clone(), s.clone()));
                view.selection = None;
                return Ok(Noop);
            }
            KeyCode::Esc => {
                view.selection = None;
                return Ok(Noop);
            }
            _ => {}
        }
    }

    if let Some((pattern, selection)) = &view.clipboard {
        if key.code == KeyCode::Char('v') && key.modifiers.contains(KeyModifiers::CONTROL) {
            let msg = app.update_pattern(|p| p.copy(view.editor.cursor, pattern, selection));
            view.clipboard = None;
            return Ok(msg);
        }
    }

    match key.code {
        KeyCode::Char('m') if key.modifiers.contains(KeyModifiers::ALT) => {
            let track = &app.state.tracks[view.editor.cursor.track()];
            return Ok(ParamToggle(track.device_id, TrackParams::MUTE));
        }
        KeyCode::Char('=') if key.modifiers.contains(KeyModifiers::ALT) => {
            let track = &app.state.tracks[view.editor.cursor.track()];
            return Ok(ParamInc(
                track.device_id,
                TrackParams::VOLUME,
                StepSize::Large,
            ));
        }
        KeyCode::Char('-') if key.modifiers.contains(KeyModifiers::ALT) => {
            let track = &app.state.tracks[view.editor.cursor.track()];
            return Ok(ParamDec(
                track.device_id,
                TrackParams::VOLUME,
                StepSize::Large,
            ));
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let pos = view.editor.cursor;
            view.selection = Some(Selection::new(pos, pos));
            return Ok(Noop);
        }
        KeyCode::Char(' ') => return Ok(TogglePlay),
        KeyCode::Backspace => {
            let msg = app.update_pattern(|p| p.clear(view.editor.cursor));
            if view.editor.cursor.is_pitch_input() {
                move_editor_cursor(app, view, CursorMove::Down);
            }
            return Ok(msg);
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            move_editor_cursor(app, view, CursorMove::Down)
        }
        KeyCode::Down => move_editor_cursor(app, view, CursorMove::Down),
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            move_editor_cursor(app, view, CursorMove::Up)
        }
        KeyCode::Up => move_editor_cursor(app, view, CursorMove::Up),
        KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            move_editor_cursor(app, view, CursorMove::Right)
        }
        KeyCode::Right => move_editor_cursor(app, view, CursorMove::Right),
        KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            move_editor_cursor(app, view, CursorMove::Left)
        }
        KeyCode::Left => move_editor_cursor(app, view, CursorMove::Left),
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            move_editor_cursor(app, view, CursorMove::LineStart)
        }
        KeyCode::Home => move_editor_cursor(app, view, CursorMove::LineStart),
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            move_editor_cursor(app, view, CursorMove::LineEnd)
        }
        KeyCode::End => move_editor_cursor(app, view, CursorMove::LineEnd),
        KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::ALT) => {
            move_editor_cursor(app, view, CursorMove::NextTrack)
        }
        KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::ALT) => {
            move_editor_cursor(app, view, CursorMove::PrevTrack)
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Ok(NextPattern)
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Ok(PrevPattern)
        }
        KeyCode::Char('[') => {
            return Ok(app.update_pattern(|p| p.incr(view.editor.cursor, StepSize::Default)))
        }
        KeyCode::Char(']') => {
            return Ok(app.update_pattern(|p| p.decr(view.editor.cursor, StepSize::Default)))
        }
        KeyCode::Char('{') => {
            return Ok(app.update_pattern(|p| p.incr(view.editor.cursor, StepSize::Large)))
        }
        KeyCode::Char('}') => {
            return Ok(app.update_pattern(|p| p.decr(view.editor.cursor, StepSize::Large)))
        }
        KeyCode::Char(key) => {
            let msg =
                app.update_pattern(|p| p.set_key(view.editor.cursor, app.state.octave as u8, key));
            if view.editor.cursor.is_pitch_input() {
                move_editor_cursor(app, view, CursorMove::Down)
            }
            return Ok(msg);
        }
        _ => {}
    }
    if let Some(s) = &mut view.selection {
        s.move_to(view.editor.cursor);
    }

    Ok(Noop)
}

fn handle_command_line_input(app: &App, view: &mut View, key: KeyEvent) -> Result<Msg> {
    use Msg::*;
    match key.code {
        KeyCode::Enter => {
            let parts: Vec<&str> = view.command.split_whitespace().collect();
            if parts.is_empty() {
                return Err(anyhow!("invalid command"));
            }
            let msg = match parts[0] {
                "oct" | "octave" => {
                    let oct: u16 = parts[1].parse()?;
                    if oct > 9 {
                        return Err(anyhow!("invalid octave: {}", oct));
                    }
                    Ok(SetOct(oct))
                }
                "bpm" => Ok(SetBpm(parts[1].parse()?)),
                "quit" | "q" | "exit" => Ok(Exit),
                "setlength" if parts.len() == 2 => {
                    let new_length = parts[1].parse()?;
                    Ok(app.update_pattern(|p| p.set_len(new_length)))
                }
                "cd" => {
                    if parts.len() > 1 {
                        Ok(ChangeDir(Utf8PathBuf::from(parts[1])))
                    } else {
                        let home = std::env::var("HOME")?;
                        if home.is_empty() {
                            Err(anyhow!("cd: invalid argument"))
                        } else {
                            Ok(ChangeDir(home.into()))
                        }
                    }
                }
                _ => Err(anyhow!("invalid command {}", parts[0])),
            };
            view.command.clear();
            view.focus = Focus::Editor;
            return msg;
        }
        KeyCode::Backspace => {
            view.command.pop();
        }
        KeyCode::Char(char) => view.command.push(char),
        KeyCode::Esc => {
            view.command.clear();
            view.focus = Focus::Editor;
        }
        _ => {}
    }

    Ok(Noop)
}

fn handle_project_tree_input(app: &App, view: &mut View, key: KeyEvent) -> Result<Msg> {
    use Msg::*;
    match key.code {
        KeyCode::Char('s') => {
            view.project_tree_state = ProjectTreeState::Instruments;
            return Ok(Noop);
        }
        KeyCode::Char('t') => {
            view.project_tree_state = ProjectTreeState::Tracks;
            return Ok(Noop);
        }
        _ => {}
    };
    match view.project_tree_state {
        ProjectTreeState::Tracks => {
            match key.code {
                KeyCode::Enter => {
                    view.project_tree_state =
                        ProjectTreeState::Devices(view.tracks.selected().unwrap())
                }
                _ => handle_list_input(&mut view.tracks, key),
            };
        }
        ProjectTreeState::InstrumentParams(instr_idx) => {
            let device_id = app.state.instruments[instr_idx].as_ref().unwrap().id;
            match key.code {
                KeyCode::Char('u') => {
                    view.project_tree_state = ProjectTreeState::Instruments;
                }
                KeyCode::Char('[') => {
                    return Ok(ParamInc(
                        device_id,
                        view.params.selected().unwrap(),
                        StepSize::Default,
                    ))
                }
                KeyCode::Char(']') => {
                    return Ok(ParamDec(
                        device_id,
                        view.params.selected().unwrap(),
                        StepSize::Default,
                    ))
                }
                KeyCode::Char('{') => {
                    return Ok(ParamInc(
                        device_id,
                        view.params.selected().unwrap(),
                        StepSize::Large,
                    ))
                }
                KeyCode::Char('}') => {
                    return Ok(ParamDec(
                        device_id,
                        view.params.selected().unwrap(),
                        StepSize::Large,
                    ))
                }
                _ => handle_list_input(&mut view.params, key),
            };
        }
        ProjectTreeState::Devices(_track_idx) => {
            match key.code {
                KeyCode::Char('u') => {
                    view.project_tree_state = ProjectTreeState::Tracks;
                }
                _ => handle_list_input(&mut view.devices, key),
            };
        }
        ProjectTreeState::Instruments => {
            match key.code {
                KeyCode::Enter => {
                    let idx = view.instruments.selected().unwrap();
                    if app.state.instruments[idx].is_some() {
                        view.project_tree_state = ProjectTreeState::InstrumentParams(idx);
                    }
                }
                KeyCode::Char('l') => view.focus = Focus::FileLoader,
                _ => handle_list_input(&mut view.instruments, key),
            };
        }
    }
    Ok(Noop)
}

enum CursorMove {
    Up,
    Down,
    Left,
    Right,
    NextTrack,
    PrevTrack,
    LineStart,
    LineEnd,
}

fn handle_list_input(list: &mut ListState, key: KeyEvent) {
    match key.code {
        KeyCode::Down => list.select_next(),
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => list.select_next(),
        KeyCode::Up => list.select_previous(),
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            list.select_previous()
        }
        _ => {}
    }
}

fn move_editor_cursor(app: &App, view: &mut View, cursor_move: CursorMove) {
    use CursorMove::*;

    let pattern_size = app.selected_pattern().size();
    let cursor = &mut view.editor.cursor;

    match cursor_move {
        Up => cursor.line = cursor.line.saturating_sub(1),
        Down => cursor.line = usize::min(pattern_size.lines - 1, cursor.line + 1),
        Left => cursor.column = cursor.column.saturating_sub(1),
        Right => cursor.column = usize::min(pattern_size.columns - 1, cursor.column + 1),
        NextTrack => {
            let col = cursor.column + INPUTS_PER_STEP;
            if col <= pattern_size.columns {
                cursor.column = col;
            }
        }
        PrevTrack => {
            if cursor.track() > 0 {
                cursor.column -= INPUTS_PER_STEP;
            }
        }
        LineStart => cursor.column = 0,
        LineEnd => cursor.column = pattern_size.columns - 1,
    }
}
