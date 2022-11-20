pub mod context;
pub mod editor;

use std::time::Duration;

use crate::app::Msg;
pub use crate::input::{Input, InputQueue};
use crate::params::ParamIterExt;
use crate::pattern;
use crate::pattern::Pattern;
use crate::pattern::Position;
use crate::pattern::Selection;
use crate::pattern::StepSize;
use crate::pattern::INPUTS_PER_STEP;
pub use crate::view::context::ViewContext;
pub use crate::view::editor::{Editor, EditorState};
use anyhow::{anyhow, Result};
use camino::Utf8PathBuf;
use termion::event::Key;
use tui::{
    backend::Backend,
    layout::Rect,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, List as ListView, ListItem, ListState, Paragraph},
    Frame,
};

const BORDER_COLOR: Color = Color::DarkGray;

pub struct View {
    frames: usize,
    cursor: Cursor,
    focus: Focus,
    files: List,
    instruments: List,
    params: List,
    tracks: List,
    devices: List,
    patterns: List,
    editor: EditorState,
    command: String,
    project_tree_state: ProjectTreeState,
    selection: Option<Selection>,
    clipboard: Option<(Pattern, Selection)>,
}

impl View {
    pub fn new() -> Self {
        Self {
            frames: 0,
            cursor: Cursor::default(),
            instruments: List::default(),
            tracks: List::default(),
            devices: List::default(),
            params: List::default(),
            patterns: List::default(),
            files: List::default(),
            editor: EditorState::default(),
            focus: Focus::Editor,
            command: String::new(),
            project_tree_state: ProjectTreeState::Instruments,
            selection: None,
            clipboard: None,
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
            .horizontal_margin(1)
            .split(screen);

        let main = sections[0];
        let status = sections[1];
        let command = sections[2];

        self.render_main(f, ctx, main);
        self.render_command_line(f, ctx, command);

        let block = Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .border_style(Style::default().fg(BORDER_COLOR));
        let area = block.inner(status);
        f.render_widget(block, status);
        self.render_status_line(f, ctx, area);
    }

    fn render_command_line<B: Backend>(&mut self, f: &mut Frame<B>, _ctx: ViewContext, area: Rect) {
        if !self.command.is_empty() {
            let spans = Spans::from(vec![Span::raw(":"), Span::raw(&*self.command)]);
            let p = Paragraph::new(spans);
            f.render_widget(p, area)
        }
    }

    fn render_status_line<B: Backend>(&mut self, f: &mut Frame<B>, ctx: ViewContext, area: Rect) {
        let style = Style::default();

        let p = Paragraph::new(format!(
            " [ {:0width$} . {:0width$} ] ",
            ctx.active_pattern_index(),
            ctx.current_line(),
            width = 3
        ))
        .alignment(Alignment::Left)
        .style(style);
        f.render_widget(p, area);

        let p = Paragraph::new("*Untitled*")
            .alignment(Alignment::Center)
            .style(style);
        f.render_widget(p, area);

        let s = format!(
            "BPM {}    LPB {}    Oct {}  ",
            ctx.bpm(),
            ctx.lines_per_beat(),
            ctx.octave()
        );
        let p = Paragraph::new(s).alignment(Alignment::Right).style(style);
        f.render_widget(p, area);
    }

    fn render_main<B: Backend>(&mut self, f: &mut Frame<B>, ctx: ViewContext, area: Rect) {
        let sections = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(80), Constraint::Percentage(20)].as_ref())
            .horizontal_margin(1)
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
        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(BORDER_COLOR));
        let area = block.inner(sections[0]);
        f.render_widget(block, sections[0]);
        self.render_patterns(f, ctx, area);

        // Editor
        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(BORDER_COLOR));
        let area = block.inner(sections[1]);
        f.render_widget(block, sections[1]);

        let editor = Editor::new(self.cursor.pos, self.focus, &self.selection, ctx);
        f.render_stateful_widget(&editor, area, &mut self.editor);
    }

    fn render_patterns<B: Backend>(&mut self, f: &mut Frame<B>, ctx: ViewContext, area: Rect) {
        let right = area.width / 2 + 2;
        let left = area.width - right;

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
                ListItem::new(Spans::from(vec![
                    play_indicator,
                    Span::raw(format!("{:width$}", i, width = 2)),
                ]))
            })
            .collect();

        let highlight_style = if self.focus == Focus::Patterns {
            Style::default().fg(Color::Black).bg(Color::Green)
        } else {
            Style::default()
        };
        let patterns = ListView::new(patterns).highlight_style(highlight_style);
        f.render_stateful_widget(patterns, sections[0], &mut self.patterns.state);

        let patterns: Vec<ListItem> = ctx
            .song2()
            .enumerate()
            .map(|(i, pattern)| {
                let selected = if i == selected_idx { ">" } else { " " };
                let looped = if ctx.loop_contains(i) { "~" } else { " " };
                ListItem::new(Spans::from(vec![
                    Span::raw(" "),
                    Span::styled("▆▆", Style::default().fg(pattern.color)),
                    Span::raw(" "),
                    Span::raw(looped),
                    Span::raw(selected),
                ]))
            })
            .collect();

        let patterns = ListView::new(patterns).block(
            Block::default()
                .borders(Borders::RIGHT)
                .border_style(Style::default().fg(BORDER_COLOR)),
        );
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
                    .tracks()
                    .iter()
                    .enumerate()
                    .map(|(i, track)| {
                        ListItem::new(Span::raw(format!(
                            "  {:0width$} {}",
                            i,
                            track.name.as_ref().map_or("-", |n| n.as_str()),
                            width = 2
                        )))
                    })
                    .collect();

                let tracks = ListView::new(tracks)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(BORDER_COLOR)),
                    )
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
                    .block(
                        Block::default()
                            .title(track_name)
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(BORDER_COLOR)),
                    )
                    .highlight_style(highlight_style);
                f.render_stateful_widget(devices, area, &mut self.devices.state);
            }
            ProjectTreeState::InstrumentParams(instrument_idx) => {
                // TODO: maybe use a table here to align values?
                let w = (area.width as f32 * 0.6) as usize;
                let params: Vec<ListItem> = ctx
                    .params(instrument_idx)
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        ListItem::new(Span::raw(format!(
                            " {:0nwidth$} {:lwidth$} {}",
                            i,
                            p.label(),
                            p.value_as_string(),
                            nwidth = 2,
                            lwidth = w
                        )))
                    })
                    .collect();
                let name: &str = ctx.instruments()[instrument_idx]
                    .as_ref()
                    .unwrap()
                    .name
                    .as_ref();

                let params = ListView::new(params)
                    .block(
                        Block::default()
                            .title(name)
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(BORDER_COLOR)),
                    )
                    .highlight_style(highlight_style);
                f.render_stateful_widget(params, area, &mut self.params.state);
            }
            ProjectTreeState::Instruments => {
                let instruments: Vec<ListItem> = ctx
                    .instruments()
                    .iter()
                    .enumerate()
                    .map(|(i, instr)| {
                        let selected =
                            if i == self.instruments.pos && self.focus != Focus::ProjectTree {
                                Span::raw(">")
                            } else {
                                Span::raw(" ")
                            };
                        let name = instr
                            .as_ref()
                            .map(|instr| instr.name.as_ref())
                            .unwrap_or("");
                        let snd_desc = Span::raw(format!(" {:0width$} {}", i, name, width = 2));
                        ListItem::new(Spans::from(vec![selected, snd_desc]))
                    })
                    .collect();
                let instruments = ListView::new(instruments)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(BORDER_COLOR)),
                    )
                    .highlight_style(highlight_style);
                f.render_stateful_widget(instruments, area, &mut self.instruments.state);
            }
        };
    }

    fn render_sidebar<B: Backend>(&mut self, f: &mut Frame<B>, ctx: ViewContext, area: Rect) {
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Ratio(1, 3), Constraint::Ratio(2, 3)].as_ref())
            .horizontal_margin(1)
            .split(area);

        self.render_project_tree(f, ctx, sections[0]);

        // File Browser
        let file_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BORDER_COLOR));
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
        let header = Paragraph::new(format!(" {}", dir)).block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(BORDER_COLOR)),
        );
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
                    let parts: Vec<&str> = self.command.split_whitespace().collect();
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
                    self.command.clear();
                    self.focus = Focus::Editor;
                    return msg;
                }
                Key::Backspace => {
                    self.command.pop();
                }
                Key::Char(char) => self.command.push(char),
                Key::Esc => self.command.clear(),
                _ => return Ok(Noop),
            }
            if key == Key::Esc || key == Key::Char('\n') {
                self.focus = Focus::Editor;
                return Ok(Noop);
            }
        }

        match self.focus {
            Focus::Editor => {
                if let Some(s) = &mut self.selection {
                    match key {
                        Key::Ctrl('y') => {
                            self.clipboard = Some((ctx.selected_pattern().clone(), s.clone()));
                            self.selection = None;
                            return Ok(Noop);
                        }
                        Key::Esc => {
                            self.selection = None;
                            return Ok(Noop);
                        }
                        _ => {}
                    }
                }

                if let Some((pattern, selection)) = &self.clipboard {
                    if let Key::Ctrl('v') = key {
                        let msg =
                            ctx.update_pattern(|p| p.copy(self.cursor.pos, pattern, selection));
                        self.clipboard = None;
                        return Ok(msg);
                    }
                }

                match key {
                    Key::Alt('m') => return Ok(ToggleMute(self.cursor.pos.track())),
                    Key::Alt('=') => return Ok(VolumeInc(Some(self.cursor.pos.track()))),
                    Key::Alt('-') => return Ok(VolumeDec(Some(self.cursor.pos.track()))),
                    Key::Ctrl('v') => {
                        let pos = self.cursor.pos;
                        self.selection = Some(Selection::new(pos, pos));
                        return Ok(Noop);
                    }
                    Key::Char(' ') => return Ok(TogglePlay),
                    Key::Backspace => {
                        let msg = ctx.update_pattern(|p| p.clear(self.cursor.pos));
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
                    Key::Alt('f') => self.cursor.next_track(),
                    Key::Alt('b') => self.cursor.prev_track(),
                    Key::Ctrl('d') => return Ok(NextPattern),
                    Key::Ctrl('u') => return Ok(PrevPattern),
                    Key::Char('[') => {
                        return Ok(
                            ctx.update_pattern(|p| p.incr(self.cursor.pos, StepSize::Default))
                        )
                    }
                    Key::Char(']') => {
                        return Ok(
                            ctx.update_pattern(|p| p.decr(self.cursor.pos, StepSize::Default))
                        )
                    }
                    Key::Char('{') => {
                        return Ok(ctx.update_pattern(|p| p.incr(self.cursor.pos, StepSize::Large)))
                    }
                    Key::Char('}') => {
                        return Ok(ctx.update_pattern(|p| p.decr(self.cursor.pos, StepSize::Large)))
                    }
                    Key::Char(key) => {
                        let msg = ctx.update_pattern(|p| {
                            p.set_key(self.cursor.pos, ctx.octave() as u8, key)
                        });
                        if self.cursor.pos.is_pitch_input() {
                            self.cursor.down()
                        }
                        return Ok(msg);
                    }
                    _ => {}
                }
                if let Some(s) = &mut self.selection {
                    s.move_to(self.cursor.pos);
                }
            }
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
                    Key::Ctrl('u') => self.instruments.prev(),
                    Key::Ctrl('d') => self.instruments.next(),
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
                            LoadSound(self.instruments.pos, selected_path.to_path_buf())
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

    fn handle_project_tree_input(&mut self, key: Key, ctx: ViewContext) -> Result<Msg> {
        use Msg::*;
        match key {
            Key::Char('s') => {
                self.project_tree_state = ProjectTreeState::Instruments;
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
            ProjectTreeState::InstrumentParams(instr_idx) => {
                let device_id = ctx.instruments()[instr_idx].as_ref().unwrap().id;
                match key {
                    Key::Char('u') => {
                        self.project_tree_state = ProjectTreeState::Instruments;
                    }
                    Key::Char('[') => {
                        return Ok(ParamInc(device_id, self.params.pos, StepSize::Default))
                    }
                    Key::Char(']') => {
                        return Ok(ParamDec(device_id, self.params.pos, StepSize::Default))
                    }
                    Key::Char('{') => {
                        return Ok(ParamInc(device_id, self.params.pos, StepSize::Large))
                    }
                    Key::Char('}') => {
                        return Ok(ParamDec(device_id, self.params.pos, StepSize::Large))
                    }
                    _ => self.params.input(key),
                };
                Ok(Noop)
            }
            ProjectTreeState::Devices(_track_idx) => {
                match key {
                    Key::Char('u') => {
                        self.project_tree_state = ProjectTreeState::Tracks;
                    }
                    _ => self.devices.input(key),
                };
                Ok(Noop)
            }
            ProjectTreeState::Instruments => {
                match key {
                    Key::Char('\n') => {
                        self.project_tree_state =
                            ProjectTreeState::InstrumentParams(self.instruments.pos);
                    }
                    Key::Char('l') => self.focus = Focus::FileLoader,
                    _ => self.instruments.input(key),
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
            ProjectTreeState::InstrumentParams(instrument) => {
                self.params.set_len(ctx.params(instrument).len());
            }
            ProjectTreeState::Devices(track) => {
                self.devices.set_len(ctx.devices(track).len());
            }
            ProjectTreeState::Instruments => {
                self.instruments.set_len(ctx.instruments().len());
            }
            ProjectTreeState::Tracks => {
                self.tracks.set_len(ctx.tracks().len());
            }
        }
        self.cursor.set_pattern_size(ctx.selected_pattern().size());
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum Focus {
    Editor,
    CommandLine,
    ProjectTree,
    FileLoader,
    Patterns,
}

enum ProjectTreeState {
    Instruments,
    Tracks,
    Devices(usize),
    InstrumentParams(usize),
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

const PATTERN_SECTION_WIDTH: usize = "> 01 XX ~>|".len();

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
    pattern_size: pattern::Rect,
}

impl Cursor {
    fn set_pattern_size(&mut self, size: pattern::Rect) {
        self.pos.line = usize::min(size.lines - 1, self.pos.line);
        self.pos.column = usize::min(size.columns - 1, self.pos.column);
        self.pattern_size = size;
    }

    fn up(&mut self) {
        self.pos.line = self.pos.line.saturating_sub(1);
    }

    fn down(&mut self) {
        self.pos.line = usize::min(self.pattern_size.lines - 1, self.pos.line + 1);
    }

    fn left(&mut self) {
        self.pos.column = self.pos.column.saturating_sub(1);
    }

    fn right(&mut self) {
        self.pos.column = usize::min(self.pattern_size.columns - 1, self.pos.column + 1);
    }

    fn next_track(&mut self) {
        let col = self.pos.column + INPUTS_PER_STEP;
        if col <= self.pattern_size.columns {
            self.pos.column = col;
        }
    }

    fn prev_track(&mut self) {
        if self.pos.track() > 0 {
            self.pos.column -= INPUTS_PER_STEP;
        }
    }

    fn start(&mut self) {
        self.pos.column = 0;
    }

    fn end(&mut self) {
        self.pos.column = self.pattern_size.columns - 1;
    }
}
