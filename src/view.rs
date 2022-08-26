pub mod editor;

use std::time::Duration;

use crate::app::{Msg, SharedState, ViewContext};
pub use crate::input::{Input, InputQueue};
use crate::pattern::Position;
use crate::pattern::StepSize;
pub use crate::view::editor::{Editor, EditorState};
use anyhow::{anyhow, Result};
use camino::Utf8PathBuf;
use termion::event::Key;
use tui::{
    backend::Backend,
    buffer::Buffer,
    layout::Rect,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Span, Spans},
    widgets::{
        Block, Borders, List as ListView, ListItem, ListState, Paragraph, StatefulWidget, Widget,
    },
    Frame,
};

pub struct View {
    frames: usize,
    cursor: Cursor,
    focus: Focus,
    files: List,
    sounds: List,
    params: List,
    tracks: List,
    devices: List,
    patterns: List,
    editor: EditorState,
    command: CommandState,
    project_tree_state: ProjectTreeState,
}

impl View {
    pub fn new() -> Self {
        Self {
            frames: 0,
            cursor: Cursor::default(),
            sounds: List::default(),
            tracks: List::default(),
            devices: List::default(),
            params: List::default(),
            patterns: List::default(),
            files: List::default(),
            editor: EditorState::default(),
            focus: Focus::Editor,
            command: CommandState {
                buffer: String::with_capacity(1024),
            },
            project_tree_state: ProjectTreeState::Sounds,
        }
    }
}

impl View {
    pub fn render<B: Backend>(&mut self, f: &mut Frame<B>, ctx: ViewContext) {
        self.set_state(ctx);
        self.frames += 1;

        let screen = f.size();
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                [
                    Constraint::Length(screen.height - 4),
                    Constraint::Length(3),
                    Constraint::Length(1),
                ]
                .as_ref(),
            )
            .split(screen);

        let main = sections[0];
        let status = sections[1];
        let command = sections[2];

        self.render_main(f, ctx, main);

        // Command input
        let command_line = CommandLine {};
        f.render_stateful_widget(command_line, command, &mut self.command);

        // Status line
        let block = Block::default().borders(Borders::all());
        let area = block.inner(status);
        f.render_widget(block, status);
        let status_line = StatusLine::new(ctx);
        f.render_widget(&status_line, area);
    }

    fn render_main<B: Backend>(&mut self, f: &mut Frame<B>, ctx: ViewContext, area: Rect) {
        let sections = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(80), Constraint::Percentage(20)].as_ref())
            .split(area);

        let editor = sections[0];
        let sidebar = sections[1];

        self.render_editor(f, ctx, editor);
        self.render_sidebar(f, ctx, sidebar);
    }

    fn render_editor<B: Backend>(&mut self, f: &mut Frame<B>, ctx: ViewContext, area: Rect) {
        let sections = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(PATTERN_SECTION_WIDTH as u16),
                Constraint::Length(area.width - PATTERN_SECTION_WIDTH as u16),
            ])
            .split(area);

        // Pattern list
        let block = Block::default().borders(Borders::all()).title("Track");
        let area = block.inner(sections[0]);
        f.render_widget(block, sections[0]);
        self.render_patterns(f, ctx, area);

        // Editor
        let block = Block::default().borders(Borders::TOP | Borders::BOTTOM);
        let area = block.inner(sections[1]);
        f.render_widget(block, sections[1]);

        let in_focus = self.focus == Focus::Editor;
        let editor = Editor::new(self.cursor.pos, in_focus, ctx);
        f.render_stateful_widget(&editor, area, &mut self.editor);
    }

    fn render_patterns<B: Backend>(&mut self, f: &mut Frame<B>, ctx: ViewContext, area: Rect) {
        let right = area.width / 2;
        let left = right + 1; // add 1 for right border

        let sections = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(left), Constraint::Length(right)])
            .split(area);

        let active_idx = ctx.active_pattern_index();
        let selected_idx = ctx.selected_pattern_index();
        let patterns: Vec<ListItem> = ctx
            .song()
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let play_indicator = if i == active_idx {
                    let style = Style::default().fg(Color::Blue);
                    if ctx.is_playing() {
                        self.animate(
                            vec![Span::styled("▶", style), Span::raw(" ")],
                            Duration::from_secs_f64(60.0 / ctx.bpm() as f64),
                        )
                    } else {
                        Span::styled("▶", style)
                    }
                } else {
                    Span::styled(" ", Style::default())
                };
                let looped = if ctx.loop_contains(i) { "~" } else { " " };
                ListItem::new(Spans::from(vec![
                    play_indicator,
                    Span::raw(" "),
                    Span::raw(format!("{:0width$}", i, width = 2)),
                    Span::raw(" "),
                    Span::raw(looped),
                ]))
            })
            .collect();

        let highlight_style = if self.focus == Focus::Patterns {
            Style::default().fg(Color::Black).bg(Color::Green)
        } else {
            Style::default()
        };
        let patterns = ListView::new(patterns)
            .highlight_style(highlight_style)
            .block(Block::default().borders(Borders::RIGHT));
        f.render_stateful_widget(patterns, sections[0], &mut self.patterns.state);

        let patterns: Vec<ListItem> = ctx
            .song()
            .iter()
            .enumerate()
            .map(|(i, pattern_id)| {
                let selected = if i == selected_idx {
                    Span::raw(" >")
                } else {
                    Span::raw("  ")
                };
                ListItem::new(Spans::from(vec![
                    Span::raw("  "),
                    Span::raw(format!("{:0width$}", pattern_id, width = 2)),
                    selected,
                ]))
            })
            .collect();

        let highlight_style = if self.focus == Focus::Patterns {
            Style::default().fg(Color::Black).bg(Color::Green)
        } else {
            Style::default()
        };
        let patterns = ListView::new(patterns).highlight_style(highlight_style);
        f.render_stateful_widget(patterns, sections[1], &mut self.patterns.state);
    }

    fn render_project_tree<B: Backend>(&mut self, f: &mut Frame<B>, ctx: ViewContext, area: Rect) {
        let highlight_style = if self.focus == Focus::ProjectTree {
            Style::default().fg(Color::Black).bg(Color::Green)
        } else {
            Style::default()
        };
        match self.project_tree_state {
            ProjectTreeState::Tracks => {
                let tracks: Vec<ListItem> = ctx
                    .iter_tracks()
                    .enumerate()
                    .map(|(i, track)| {
                        ListItem::new(Span::raw(format!(
                            "  {:0width$} {}",
                            i,
                            track.name.unwrap_or("-"),
                            width = 2
                        )))
                    })
                    .collect();

                let tracks = ListView::new(tracks)
                    .block(Block::default().borders(Borders::ALL).title("Tracks"))
                    .highlight_style(highlight_style);
                f.render_stateful_widget(tracks, area, &mut self.tracks.state);
            }
            ProjectTreeState::Devices(track_idx) => {
                let devices: Vec<ListItem> = ctx
                    .devices(track_idx)
                    .iter()
                    .enumerate()
                    .map(|(i, dev)| {
                        ListItem::new(Span::raw(format!(" {:0width$} {}", i, dev.name, width = 2)))
                    })
                    .collect();

                let track_name = ctx.tracks()[track_idx]
                    .name
                    .clone()
                    .unwrap_or(format!("Track {track_idx}"));
                let devices = ListView::new(devices)
                    .block(Block::default().borders(Borders::ALL).title(track_name))
                    .highlight_style(highlight_style);
                f.render_stateful_widget(devices, area, &mut self.devices.state);
            }
            ProjectTreeState::DeviceParams(track_idx, device_idx) => {
                // TODO: maybe use a table here to align values?
                let w = (area.width as f32 * 0.6) as usize;
                let params: Vec<ListItem> = ctx
                    .params(track_idx, device_idx)
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        ListItem::new(Span::raw(format!(
                            " {:0nwidth$} {:lwidth$} {}",
                            i,
                            p.label,
                            p.value,
                            nwidth = 2,
                            lwidth = w
                        )))
                    })
                    .collect();
                let device_name: &str = ctx.devices(track_idx)[device_idx].name.as_ref();
                let track_name = ctx.tracks()[track_idx]
                    .name
                    .clone()
                    .unwrap_or(format!("Track {track_idx}"));

                let params = ListView::new(params)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(format!("{track_name} > {device_name}")),
                    )
                    .highlight_style(highlight_style);
                f.render_stateful_widget(params, area, &mut self.params.state);
            }
            ProjectTreeState::Sounds => {
                // Sounds
                let sounds: Vec<ListItem> = ctx
                    .sounds()
                    .iter()
                    .enumerate()
                    .map(|(i, snd)| {
                        let selected = if i == self.sounds.pos && self.focus != Focus::ProjectTree {
                            Span::raw(">")
                        } else {
                            Span::raw(" ")
                        };
                        let snd_desc = snd
                            .as_ref()
                            .map_or("", |snd| snd.path.file_name().unwrap_or(""));
                        let snd_desc = Span::raw(format!(" {:0width$} {}", i, snd_desc, width = 2));
                        ListItem::new(Spans::from(vec![selected, snd_desc]))
                    })
                    .collect();
                let sounds = ListView::new(sounds)
                    .block(Block::default().borders(Borders::ALL).title("Sounds"))
                    .highlight_style(highlight_style);
                f.render_stateful_widget(sounds, area, &mut self.sounds.state);
            }
        };
    }

    fn render_sidebar<B: Backend>(&mut self, f: &mut Frame<B>, ctx: ViewContext, area: Rect) {
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Ratio(1, 3), Constraint::Ratio(2, 3)].as_ref())
            .split(area);

        self.render_project_tree(f, ctx, sections[0]);

        // File Browser
        let file_block = Block::default().borders(Borders::all()).title("Files");
        let file_sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                [
                    Constraint::Length(sections[1].height - 4),
                    Constraint::Length(4),
                ]
                .as_ref(),
            )
            .split(file_block.inner(sections[1]));

        f.render_widget(file_block, sections[1]);

        let highlight_style = if self.focus == Focus::FileLoader {
            Style::default().fg(Color::Black).bg(Color::Green)
        } else {
            Style::default()
        };
        let files: Vec<ListItem> = ctx
            .file_browser
            .entries
            .iter()
            .map(|path| ListItem::new(Span::raw(format!(" {}", path.file_name().unwrap_or("")))))
            .collect();
        let files = ListView::new(files).highlight_style(highlight_style);
        f.render_stateful_widget(files, file_sections[0], &mut self.files.state);

        let dir = shorten_path(&ctx.file_browser.dir, file_sections[1].width as usize - 8);
        let header =
            Paragraph::new(format!(" {}", dir)).block(Block::default().borders(Borders::TOP));
        f.render_widget(header, file_sections[1]);
    }

    pub fn handle_input(&mut self, key: Key, ctx: ViewContext) -> Msg {
        match self.handle_input_inner(key, ctx) {
            Ok(change) => change,
            Err(err) => {
                eprintln!("error: {}", err);
                Msg::Noop
            }
        }
    }

    fn handle_input_inner(&mut self, key: Key, ctx: ViewContext) -> Result<Msg> {
        use Msg::*;

        if key == Key::Ctrl('w') {
            use Focus::*;
            self.focus = match self.focus {
                Patterns => Editor,
                Editor => ProjectTree,
                ProjectTree => FileLoader,
                FileLoader => Patterns,
                CommandLine => CommandLine,
            };
            return Ok(Noop);
        }

        if key == Key::Char(':') && self.focus != Focus::CommandLine {
            self.focus = Focus::CommandLine;
            return Ok(Noop);
        }

        if self.focus == Focus::CommandLine {
            match key {
                Key::Char('\n') => {
                    let parts: Vec<&str> = self.command.buffer.split_whitespace().collect();
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
                        "setlength" if parts.len() == 2 => Ok(SetPatternLen(parts[1].parse()?)),
                        "volume" => {
                            let cmd = if parts.len() == 3 {
                                let track: usize = parts[1].parse()?;
                                let value: f64 = parts[2].parse()?;
                                SetVolume(Some(track), value)
                            } else {
                                let value: f64 = parts[1].parse()?;
                                SetVolume(None, value)
                            };
                            Ok(cmd)
                        }
                        _ => Err(anyhow!("invalid command {}", parts[0])),
                    };
                    self.command.buffer.clear();
                    self.focus = Focus::Editor;
                    return msg;
                }
                Key::Backspace => {
                    self.command.buffer.pop();
                }
                Key::Char(char) => self.command.buffer.push(char),
                Key::Esc => self.command.buffer.clear(),
                _ => return Ok(Noop),
            }
            if key == Key::Esc || key == Key::Char('\n') {
                self.focus = Focus::Editor;
                return Ok(Noop);
            }
        }

        match self.focus {
            Focus::Editor => match key {
                Key::Alt('m') => return Ok(ToggleMute(self.cursor.pos.track())),
                Key::Alt('=') => return Ok(VolumeInc(Some(self.cursor.pos.track()))),
                Key::Alt('-') => return Ok(VolumeDec(Some(self.cursor.pos.track()))),
                Key::Char(' ') => return Ok(TogglePlay),
                Key::Backspace => {
                    let step = ctx.update_step(self.cursor.pos, |mut s| s.clear());
                    let msg = SetPatternStep(self.cursor.pos, step);
                    if self.cursor.pos.is_pitch_input() {
                        self.cursor.down();
                    }
                    return Ok(msg);
                }
                Key::Ctrl('n') | Key::Down => self.cursor.down(),
                Key::Ctrl('p') | Key::Up => self.cursor.up(),
                Key::Ctrl('f') | Key::Right => self.cursor.right(),
                Key::Ctrl('b') | Key::Left => self.cursor.left(),
                Key::Ctrl('a') | Key::Home => self.cursor.start(),
                Key::Ctrl('e') | Key::End => self.cursor.end(),
                Key::Ctrl('d') => return Ok(NextPattern),
                Key::Ctrl('u') => return Ok(PrevPattern),
                Key::Char('[') => {
                    return Ok(SetPatternStep(
                        self.cursor.pos,
                        ctx.update_step(self.cursor.pos, |mut s| s.next(StepSize::Default)),
                    ))
                }
                Key::Char(']') => {
                    return Ok(SetPatternStep(
                        self.cursor.pos,
                        ctx.update_step(self.cursor.pos, |mut s| s.prev(StepSize::Default)),
                    ))
                }
                Key::Char('{') => {
                    return Ok(SetPatternStep(
                        self.cursor.pos,
                        ctx.update_step(self.cursor.pos, |mut s| s.next(StepSize::Large)),
                    ))
                }
                Key::Char('}') => {
                    return Ok(SetPatternStep(
                        self.cursor.pos,
                        ctx.update_step(self.cursor.pos, |mut s| s.prev(StepSize::Large)),
                    ))
                }
                Key::Char(key) => {
                    let step = ctx.update_step(self.cursor.pos, |mut s| s.keypress(ctx, key));
                    let msg = SetPatternStep(self.cursor.pos, step);
                    if self.cursor.pos.is_pitch_input() {
                        self.cursor.down()
                    }
                    return Ok(msg);
                }
                _ => {}
            },
            Focus::CommandLine => {}
            Focus::Patterns => match key {
                Key::Backspace => return Ok(DeletePattern(self.patterns.pos)),
                Key::Ctrl('c') => return Ok(CreatePattern(Some(self.patterns.pos))),
                Key::Ctrl('r') => return Ok(RepeatPattern(self.patterns.pos)),
                Key::Ctrl('d') => return Ok(ClonePattern(self.patterns.pos)),
                Key::Char('l') => return Ok(LoopToggle(self.patterns.pos)),
                Key::Char('L') => return Ok(LoopAdd(self.patterns.pos)),
                Key::Char('\n') => return Ok(SelectPattern(self.patterns.pos)),
                _ => self.patterns.input(key),
            },
            Focus::ProjectTree => return self.handle_project_tree_input(key, ctx),
            Focus::FileLoader => {
                match key {
                    Key::Ctrl('u') => self.sounds.next(),
                    Key::Ctrl('d') => self.sounds.prev(),
                    Key::Char('u') => {
                        if let Some(dir) = ctx.file_browser.dir.parent() {
                            self.files = List::default();
                            return Ok(ChangeDir(dir.to_path_buf()));
                        }
                    }
                    Key::Char(' ') => {
                        let selected_path = &ctx.file_browser.entries[self.files.pos];
                        return Ok(PreviewSound(selected_path.to_path_buf()));
                    }
                    Key::Char('\n') => {
                        let selected_path = &ctx.file_browser.entries[self.files.pos];
                        let msg = if selected_path.is_dir() {
                            self.files = List::default();
                            ChangeDir(selected_path.to_path_buf())
                        } else {
                            LoadSound(self.sounds.pos, selected_path.to_path_buf())
                        };
                        return Ok(msg);
                    }
                    _ => self.files.input(key),
                };
                return Ok(Noop);
            }
        }

        Ok(Noop)
    }

    fn handle_project_tree_input(&mut self, key: Key, _ctx: ViewContext) -> Result<Msg> {
        use Msg::*;
        match key {
            Key::Char('s') => {
                self.project_tree_state = ProjectTreeState::Sounds;
                return Ok(Noop);
            }
            Key::Char('t') => {
                self.project_tree_state = ProjectTreeState::Tracks;
                return Ok(Noop);
            }
            _ => {}
        };
        match self.project_tree_state {
            ProjectTreeState::Tracks => {
                match key {
                    Key::Char('\n') => {
                        self.project_tree_state = ProjectTreeState::Devices(self.tracks.pos)
                    }
                    _ => self.tracks.input(key),
                };
                Ok(Noop)
            }
            ProjectTreeState::DeviceParams(track_idx, device_idx) => {
                match key {
                    Key::Char('u') => {
                        self.project_tree_state = ProjectTreeState::Devices(track_idx)
                    }
                    Key::Char('[') => {
                        return Ok(ParamInc(
                            track_idx,
                            device_idx,
                            self.params.pos,
                            StepSize::Default,
                        ))
                    }
                    Key::Char(']') => {
                        return Ok(ParamDec(
                            track_idx,
                            device_idx,
                            self.params.pos,
                            StepSize::Default,
                        ))
                    }
                    Key::Char('{') => {
                        return Ok(ParamInc(
                            track_idx,
                            device_idx,
                            self.params.pos,
                            StepSize::Large,
                        ))
                    }
                    Key::Char('}') => {
                        return Ok(ParamDec(
                            track_idx,
                            device_idx,
                            self.params.pos,
                            StepSize::Large,
                        ))
                    }
                    _ => self.params.input(key),
                };
                Ok(Noop)
            }
            ProjectTreeState::Devices(track_idx) => {
                match key {
                    Key::Char('\n') => {
                        self.project_tree_state =
                            ProjectTreeState::DeviceParams(track_idx, self.devices.pos);
                    }
                    Key::Char('u') => {
                        self.project_tree_state = ProjectTreeState::Tracks;
                    }
                    _ => self.devices.input(key),
                };
                Ok(Noop)
            }
            ProjectTreeState::Sounds => {
                match key {
                    Key::Char('l') => self.focus = Focus::FileLoader,
                    _ => self.sounds.input(key),
                };
                Ok(Noop)
            }
        }
    }

    fn animate<'a>(&self, states: Vec<Span<'a>>, state_dur: Duration) -> Span<'a> {
        let elapsed = self.frames as f64 / 30.0;
        let period = elapsed / state_dur.as_secs_f64();
        states[period.ceil() as usize % states.len()].clone()
    }

    fn set_state(&mut self, ctx: ViewContext) {
        self.patterns.set_len(ctx.song().len());
        self.files.set_len(ctx.file_browser.entries.len());
        match self.project_tree_state {
            ProjectTreeState::DeviceParams(track, device) => {
                self.params.set_len(ctx.params(track, device).len());
            }
            ProjectTreeState::Devices(track) => {
                self.devices.set_len(ctx.devices(track).len());
            }
            ProjectTreeState::Sounds => {
                self.sounds.set_len(ctx.sounds().len());
            }
            ProjectTreeState::Tracks => {
                self.tracks.set_len(ctx.tracks().len());
            }
        }
        self.cursor.set_pattern_size(ctx.selected_pattern().size());
    }
}

#[derive(PartialEq, Debug)]
enum Focus {
    Editor,
    CommandLine,
    ProjectTree,
    FileLoader,
    Patterns,
}

enum ProjectTreeState {
    Sounds,
    Tracks,
    Devices(usize),
    DeviceParams(usize, usize),
}

struct StatusLine {
    bpm: u16,
    lines_per_beat: u16,
    octave: u16,
    pattern_index: usize,
    line: usize,
}

impl StatusLine {
    fn new(ctx: ViewContext) -> Self {
        Self {
            bpm: ctx.bpm(),
            lines_per_beat: ctx.lines_per_beat(),
            octave: ctx.octave(),
            pattern_index: ctx.active_pattern_index(),
            line: ctx.current_line(),
        }
    }
}
impl Widget for &StatusLine {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let s = format!(
            " *Untitled*    BPM {}    LPB {}    Oct {}",
            self.bpm, self.lines_per_beat, self.octave
        );
        let p = Paragraph::new(s).alignment(Alignment::Left);
        p.render(area, buf);

        let p = Paragraph::new(format!(
            "[ {:0width$} . {:0width$} ]   ",
            self.pattern_index,
            self.line,
            width = 3
        ))
        .alignment(Alignment::Right);
        p.render(area, buf);
    }
}

struct CommandLine {}

struct CommandState {
    buffer: String,
}

impl StatefulWidget for CommandLine {
    type State = CommandState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if !state.buffer.is_empty() {
            buf.set_string(area.left(), area.top(), ":", Style::default());
            buf.set_string(area.left() + 1, area.top(), &state.buffer, Style::default());
        }
    }
}

fn shorten_path(path: &Utf8PathBuf, width: usize) -> String {
    let str = path.as_str();
    if str.len() > width {
        let elipsis = "..";
        let start = str.len() - (width + elipsis.len());
        // TODO: slice at component boundary
        format!("{}{}", elipsis, &str[start..])
    } else {
        String::from(str)
    }
}

const PATTERN_SECTION_WIDTH: usize = "|> 01 ~|  01  |".len();

struct List {
    pos: usize,
    len: usize,
    state: ListState,
}

impl List {
    fn next(&mut self) {
        self.pos = usize::min(self.pos + 1, self.len - 1);
        self.state.select(Some(self.pos));
    }

    fn prev(&mut self) {
        self.pos = self.pos.saturating_sub(1);
        self.state.select(Some(self.pos));
    }

    fn set_len(&mut self, len: usize) {
        self.len = len;
        self.pos = usize::min(self.len - 1, self.pos);
        self.state.select(Some(self.pos));
    }

    fn input(&mut self, key: Key) {
        match key {
            Key::Down | Key::Ctrl('n') => self.next(),
            Key::Up | Key::Ctrl('p') => self.prev(),
            _ => {}
        }
    }
}

impl Default for List {
    fn default() -> Self {
        let mut list = Self {
            pos: 0,
            len: 0,
            state: ListState::default(),
        };
        list.state.select(Some(0));
        list
    }
}

#[derive(Default)]
struct Cursor {
    pos: Position,
    pattern_size: (usize, usize),
}

impl Cursor {
    fn set_pattern_size(&mut self, size: (usize, usize)) {
        self.pattern_size = size;
        self.pos.line = usize::min(size.0 - 1, self.pos.line);
        self.pos.column = usize::min(size.1 - 1, self.pos.column);
    }

    fn up(&mut self) {
        self.pos.line = self.pos.line.saturating_sub(1);
    }

    fn down(&mut self) {
        self.pos.line = usize::min(self.pattern_size.0 - 1, self.pos.line + 1);
    }

    fn left(&mut self) {
        self.pos.column = self.pos.column.saturating_sub(1);
    }

    fn right(&mut self) {
        self.pos.column = usize::min(self.pattern_size.1 - 1, self.pos.column + 1);
    }

    fn start(&mut self) {
        self.pos.column = 0;
    }

    fn end(&mut self) {
        self.pos.column = self.pattern_size.1 - 1;
    }
}
