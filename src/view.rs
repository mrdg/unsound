pub mod editor;

use std::time::Duration;

use crate::app::{Msg, SharedState, ViewContext};
pub use crate::input::{Input, InputQueue};
use crate::pattern::InputType;
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
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget},
    Frame,
};

pub struct View {
    frames: usize,
    cursor: Position,
    focus: Focus,
    files: ListState,
    current_dir: Utf8PathBuf,
    sounds: ListState,
    patterns: ListState,
    editor: EditorState,
    command: CommandState,
}

impl View {
    pub fn new() -> Self {
        let mut file_state = ListState::default();
        file_state.select(Some(0));
        let mut pattern_state = ListState::default();
        pattern_state.select(Some(0));

        Self {
            frames: 0,
            cursor: Position::default(),
            sounds: ListState::default(),
            patterns: pattern_state,
            files: file_state,
            current_dir: Utf8PathBuf::new(),
            editor: EditorState::default(),
            focus: Focus::Editor,
            command: CommandState {
                buffer: String::with_capacity(1024),
            },
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
        let title = format!(" Pattern {} ", ctx.selected_pattern_index());
        let block = Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .title(title);
        let area = block.inner(sections[1]);
        f.render_widget(block, sections[1]);

        let in_focus = self.focus == Focus::Editor;
        let editor = Editor::new(self.cursor, in_focus, ctx);
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
        let patterns = List::new(patterns)
            .highlight_style(highlight_style)
            .block(Block::default().borders(Borders::RIGHT));
        f.render_stateful_widget(patterns, sections[0], &mut self.patterns);

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
        let patterns = List::new(patterns).highlight_style(highlight_style);
        f.render_stateful_widget(patterns, sections[1], &mut self.patterns);
    }

    fn render_sidebar<B: Backend>(&mut self, f: &mut Frame<B>, ctx: ViewContext, area: Rect) {
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Ratio(1, 3), Constraint::Ratio(2, 3)].as_ref())
            .split(area);

        // Sounds
        let sounds: Vec<ListItem> = ctx
            .sounds()
            .iter()
            .enumerate()
            .map(|(i, snd)| {
                ListItem::new(Span::raw(format!(
                    " {:0width$} {}",
                    i,
                    snd.as_ref()
                        .map_or("", |snd| snd.path.file_name().unwrap_or("")),
                    width = 2
                )))
            })
            .collect();

        let sounds = List::new(sounds)
            .block(Block::default().title("Sounds").borders(Borders::all()))
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Green));

        f.render_stateful_widget(sounds, sections[0], &mut self.sounds);

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

        let highlight_style = if self.focus == Focus::FileBrowser {
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
        let files = List::new(files).highlight_style(highlight_style);
        f.render_stateful_widget(files, file_sections[0], &mut self.files);

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
                Editor => FileBrowser,
                FileBrowser => Patterns,
                CommandLine => CommandLine,
            };
            return Ok(Noop);
        }

        if key == Key::Char(':') {
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
                Key::Alt('m') => return Ok(ToggleMute(self.cursor.track())),
                Key::Char(' ') => return Ok(TogglePlay),
                Key::Backspace => {
                    let change = DeleteNoteValue(self.cursor);
                    self.move_cursor(ctx, CursorMove::Down);
                    return Ok(change);
                }
                Key::Ctrl('n') | Key::Down => self.move_cursor(ctx, CursorMove::Down),
                Key::Ctrl('p') | Key::Up => self.move_cursor(ctx, CursorMove::Up),
                Key::Ctrl('f') | Key::Right => self.move_cursor(ctx, CursorMove::Right),
                Key::Ctrl('b') | Key::Left => self.move_cursor(ctx, CursorMove::Left),
                Key::Ctrl('a') | Key::Home => self.move_cursor(ctx, CursorMove::Start),
                Key::Ctrl('e') | Key::End => self.move_cursor(ctx, CursorMove::End),
                Key::Ctrl('d') => return Ok(NextPattern),
                Key::Ctrl('u') => return Ok(PrevPattern),
                Key::Char('=') => return Ok(VolumeInc(Some(self.cursor.track()))),
                Key::Char('-') => return Ok(VolumeDec(Some(self.cursor.track()))),
                Key::Char('[') => return Ok(PatternInc(self.cursor, StepSize::Default)),
                Key::Char(']') => return Ok(PatternDec(self.cursor, StepSize::Default)),
                Key::Char('{') => return Ok(PatternInc(self.cursor, StepSize::Large)),
                Key::Char('}') => return Ok(PatternDec(self.cursor, StepSize::Large)),
                Key::Char(key) => match self.cursor.input_type() {
                    InputType::Pitch => {
                        if let Some(change) = set_pitch(self.cursor, key) {
                            self.move_cursor(ctx, CursorMove::Down);
                            return Ok(change);
                        }
                        return Ok(Noop);
                    }
                    InputType::Sound => return Ok(set_sound(self.cursor, key)),
                },
                _ => {}
            },
            Focus::CommandLine => {}
            Focus::Patterns => {
                let num_patterns = ctx.song().len();
                match key {
                    Key::Down | Key::Ctrl('n') => self.patterns.move_cursor(1, num_patterns),
                    Key::Up | Key::Ctrl('p') => self.patterns.move_cursor(-1, num_patterns),
                    Key::Backspace => return Ok(DeletePattern(self.patterns.cursor())),
                    Key::Ctrl('c') => return Ok(CreatePattern(Some(self.patterns.cursor()))),
                    Key::Ctrl('r') => return Ok(RepeatPattern(self.patterns.cursor())),
                    Key::Ctrl('d') => return Ok(ClonePattern(self.patterns.cursor())),
                    Key::Char('l') => return Ok(LoopToggle(self.patterns.cursor())),
                    Key::Char('L') => return Ok(LoopAdd(self.patterns.cursor())),
                    Key::Char('\n') => return Ok(SelectPattern(self.patterns.cursor())),
                    _ => {}
                }
            }
            Focus::FileBrowser => {
                let file_count = ctx.file_browser.entries.len();
                let change = match key {
                    Key::Down | Key::Ctrl('n') => {
                        self.files.move_cursor(1, file_count);
                        Noop
                    }
                    Key::Up | Key::Ctrl('p') => {
                        self.files.move_cursor(-1, file_count);
                        Noop
                    }
                    Key::Char('[') => {
                        if let Some(dir) = ctx.file_browser.dir.parent() {
                            self.files.select_first();
                            ChangeDir(dir.to_path_buf())
                        } else {
                            Noop
                        }
                    }
                    Key::Char(' ') => {
                        let selected_path = &ctx.file_browser.entries[self.files.cursor()];
                        PreviewSound(selected_path.to_path_buf())
                    }
                    Key::Char('\n') => {
                        let selected_path = &ctx.file_browser.entries[self.files.cursor()];
                        if selected_path.is_dir() {
                            self.files.select_first();
                            ChangeDir(selected_path.to_path_buf())
                        } else {
                            LoadSound(self.cursor.track(), selected_path.to_path_buf())
                        }
                    }
                    _ => Noop,
                };
                return Ok(change);
            }
        }

        Ok(Noop)
    }

    fn move_cursor(&mut self, ctx: ViewContext, m: CursorMove) {
        let (height, width) = ctx.selected_pattern().size();
        let cursor = &mut self.cursor;
        use CursorMove::*;
        match m {
            Left => cursor.column = cursor.column.saturating_sub(1),
            Right => cursor.column = usize::min(cursor.column + 1, width - 1),
            Start => cursor.column = 0,
            End => cursor.column = width - 1,
            Up => cursor.line = cursor.line.saturating_sub(1),
            Down => cursor.line = usize::min(height - 1, cursor.line + 1),
        }
    }

    fn animate<'a>(&self, states: Vec<Span<'a>>, state_dur: Duration) -> Span<'a> {
        let elapsed = self.frames as f64 / 30.0;
        let period = elapsed / state_dur.as_secs_f64();
        states[period.ceil() as usize % states.len()].clone()
    }

    fn set_state(&mut self, ctx: ViewContext) {
        if self.patterns.cursor() >= ctx.song().len() {
            self.patterns.select(Some(ctx.song().len() - 1));
        }

        let pattern_size = ctx.selected_pattern().size();
        self.cursor.clamp(pattern_size);

        self.current_dir.clear();
        for comp in ctx.file_browser.dir.components() {
            let first_char = comp.as_str().chars().next().unwrap_or(' ');
            self.current_dir.push(first_char.to_string());
        }
        if let Some(file_name) = ctx.file_browser.dir.file_name() {
            self.current_dir.set_file_name(file_name);
        }
    }
}

enum CursorMove {
    Left,
    Right,
    Up,
    Down,
    Start,
    End,
}

#[derive(PartialEq)]
enum Focus {
    Editor,
    CommandLine,
    FileBrowser,
    Patterns,
}

fn set_sound(pos: Position, key: char) -> Msg {
    if let Some(num) = key.to_digit(10) {
        Msg::SetSound(pos, num as i32)
    } else {
        Msg::Noop
    }
}

fn set_pitch(pos: Position, key: char) -> Option<Msg> {
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
        _ => return None,
    };
    Some(Msg::SetPitch(pos, pitch as u8))
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

impl ListCursorExt for ListState {
    fn selected(&self) -> Option<usize> {
        self.selected()
    }

    fn select(&mut self, index: Option<usize>) {
        self.select(index)
    }
}

pub trait ListCursorExt {
    fn selected(&self) -> Option<usize>;
    fn select(&mut self, index: Option<usize>);

    fn select_first(&mut self) {
        self.select(None); // ensures offset gets reset
        self.select(Some(0));
    }

    fn move_cursor(&mut self, step: isize, max: usize) {
        let max = max as isize;
        if let Some(curr) = self.selected() {
            let mut new = curr as isize + step;
            if new >= max {
                new -= max;
            } else if new < 0 {
                new += max;
            }
            self.select(Some(new as usize));
        } else {
            self.select(Some(0));
        }
    }

    fn cursor(&self) -> usize {
        self.selected().unwrap_or(0)
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
