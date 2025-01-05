pub mod editor;

use std::time::Duration;

use camino::Utf8PathBuf;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List as ListView, ListItem, Paragraph, Widget},
    Frame,
};

use crate::app::App;
use crate::input::{Cursor, List};
use crate::params::ParamIterExt;
use crate::pattern::{Pattern, Selection};
use crate::sampler;
use crate::view::editor::EditorState;

const BORDER_COLOR: Color = Color::DarkGray;
const PATTERN_SECTION_WIDTH: usize = "> 01 XX ~>|".len();

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum Focus {
    Editor,
    CommandLine,
    ProjectTree,
    FileLoader,
    Patterns,
}

pub enum ProjectTreeState {
    Instruments,
    Tracks,
    Devices(usize),
    InstrumentParams(usize),
}

pub struct View {
    pub cursor: Cursor,
    pub focus: Focus,
    pub files: List,
    pub instruments: List,
    pub params: List,
    pub tracks: List,
    pub devices: List,
    pub patterns: List,
    pub project_tree_state: ProjectTreeState,
    pub selection: Option<Selection>,
    pub clipboard: Option<(Pattern, Selection)>,
    pub command: String,

    editor: EditorState,
    frames: usize,
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

pub fn render(app: &App, view: &mut View, f: &mut Frame) {
    view.cursor
        .set_pattern_size(app.state.selected_pattern().size());
    view.frames += 1;

    let screen = f.area();
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

    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(80), Constraint::Percentage(20)].as_ref())
        .horizontal_margin(1)
        .split(main);

    let editor = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(PATTERN_SECTION_WIDTH as u16),
            Constraint::Length(main[0].width - PATTERN_SECTION_WIDTH as u16),
        ])
        .split(main[0]);

    let sidebar = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Ratio(1, 3), Constraint::Ratio(2, 3)].as_ref())
        .horizontal_margin(1)
        .split(main[1]);

    let area = render_outer_block(f.buffer_mut(), editor[0], Borders::TOP);
    render_patterns(app, view, f, area);

    let area = render_outer_block(f.buffer_mut(), editor[1], Borders::TOP);
    editor::render(app, view, area, f.buffer_mut());

    render_project_tree(app, view, f, sidebar[0]);
    render_file_browser(app, view, f, sidebar[1]);

    if !view.command.is_empty() {
        let spans = Line::from(vec![Span::raw(":"), Span::raw(&*view.command)]);
        let paragraph = Paragraph::new(spans);
        f.render_widget(paragraph, command)
    }

    let area = render_outer_block(f.buffer_mut(), status, Borders::TOP | Borders::BOTTOM);
    render_status_line(app, view, f, area);
}

fn render_patterns(app: &App, view: &mut View, f: &mut Frame, area: Rect) {
    view.patterns.set_len(app.state.song.len());
    let right = area.width / 2 + 2;
    let left = area.width - right;

    let sections = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(left), Constraint::Length(right)])
        .split(area);

    let patterns: Vec<ListItem> = app
        .state
        .song
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let selected = if i == app.state.selected_pattern {
                ">"
            } else {
                " "
            };
            ListItem::new(Line::from(vec![
                Span::raw(selected),
                Span::raw(format!("{:width$}", i, width = 2)),
            ]))
        })
        .collect();

    let highlight_style = highlight_style(view, Focus::Patterns);
    let patterns = ListView::new(patterns).highlight_style(highlight_style);
    f.render_stateful_widget(patterns, sections[0], &mut view.patterns.state);

    let patterns: Vec<ListItem> = app
        .state
        .song_iter()
        .enumerate()
        .map(|(i, pattern)| {
            let looped = if app.state.loop_contains(i) { "~" } else { " " };
            let play_indicator = if i == app.engine_state.current_pattern {
                let style = Style::default().fg(Color::Blue);
                if app.state.is_playing {
                    animate(
                        view,
                        vec![Span::styled("▶", style), Span::raw(" ")],
                        Duration::from_secs_f64(60.0 / app.state.bpm as f64),
                    )
                } else {
                    Span::styled("▶", style)
                }
            } else {
                Span::styled(" ", Style::default())
            };
            ListItem::new(Line::from(vec![
                Span::raw(" "),
                Span::styled("▆▆", Style::default().fg(pattern.color)),
                Span::raw(" "),
                Span::styled(looped, Style::default().fg(Color::Blue)),
                play_indicator,
            ]))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(BORDER_COLOR));
    let patterns = ListView::new(patterns).block(block);
    f.render_stateful_widget(patterns, sections[1], &mut view.patterns.state);
}

fn render_status_line(app: &App, _view: &mut View, f: &mut Frame, area: Rect) {
    let playback_position = format!(
        " [ {:0width$} . {:0width$} ] ",
        app.engine_state.current_pattern,
        app.engine_state.current_line(),
        width = 3
    );
    let paragraph = Paragraph::new(playback_position).alignment(Alignment::Left);
    f.render_widget(paragraph, area);

    let paragraph = Paragraph::new("*Untitled*").alignment(Alignment::Center);
    f.render_widget(paragraph, area);

    let settings = format!(
        "BPM {}    LPB {}    Oct {}  ",
        app.state.bpm, app.state.lines_per_beat, app.state.octave,
    );
    let paragraph = Paragraph::new(settings).alignment(Alignment::Right);
    f.render_widget(paragraph, area);
}

fn render_project_tree(app: &App, view: &mut View, f: &mut Frame, area: Rect) {
    let highlight_style = highlight_style(view, Focus::ProjectTree);
    match view.project_tree_state {
        ProjectTreeState::Tracks => {
            view.tracks.set_len(app.state.tracks.len());
            let tracks: Vec<ListItem> = app
                .state
                .tracks
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
            f.render_stateful_widget(tracks, area, &mut view.tracks.state);
        }
        ProjectTreeState::Devices(track_idx) => {
            let devices = &app.state.tracks[track_idx].effects;
            view.devices.set_len(devices.len());
            let devices: Vec<ListItem> = devices
                .iter()
                .enumerate()
                .map(|(i, dev)| {
                    ListItem::new(Span::raw(format!(" {:0width$} {}", i, dev.name, width = 2)))
                })
                .collect();

            let track_name = app.state.tracks[track_idx]
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
            f.render_stateful_widget(devices, area, &mut view.devices.state);
        }
        ProjectTreeState::InstrumentParams(instrument_idx) => {
            let instrument = app.state.instruments[instrument_idx].as_ref().unwrap();
            let params = app.params(instrument.id);
            view.params.set_len(params.len());

            // TODO: maybe use a table here to align values?
            let w = (area.width as f32 * 0.6) as usize;
            let params: Vec<ListItem> = params
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    ListItem::new(Span::raw(format!(
                        " {:0nwidth$} {:lwidth$} {}",
                        i,
                        p.label(),
                        p.as_string(),
                        nwidth = 2,
                        lwidth = w
                    )))
                })
                .collect();

            let params = ListView::new(params)
                .block(
                    Block::default()
                        .title(instrument.name.as_ref())
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(BORDER_COLOR)),
                )
                .highlight_style(highlight_style);
            f.render_stateful_widget(params, area, &mut view.params.state);
        }
        ProjectTreeState::Instruments => {
            view.instruments.set_len(app.state.instruments.len());
            let instruments: Vec<ListItem> = app
                .state
                .instruments
                .iter()
                .enumerate()
                .map(|(i, instr)| {
                    let selected = if i == view.instruments.pos && view.focus != Focus::ProjectTree
                    {
                        Span::raw(">")
                    } else {
                        Span::raw(" ")
                    };
                    let name = instr
                        .as_ref()
                        .map(|instr| instr.name.as_ref())
                        .unwrap_or("");
                    let snd_desc = Span::raw(format!(" {:0width$} {}", i, name, width = 2));
                    ListItem::new(Line::from(vec![selected, snd_desc]))
                })
                .collect();
            let instruments = ListView::new(instruments)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(BORDER_COLOR)),
                )
                .highlight_style(highlight_style);
            f.render_stateful_widget(instruments, area, &mut view.instruments.state);
        }
    };
}

fn render_file_browser(app: &App, view: &mut View, f: &mut Frame, area: Rect) {
    view.files.set_len(app.file_browser.entries.len());

    let area = render_outer_block(f.buffer_mut(), area, Borders::ALL);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(area.height - 2), Constraint::Length(2)].as_ref())
        .split(area);

    let highlight_style = highlight_style(view, Focus::FileLoader);
    let files: Vec<ListItem> = app
        .file_browser
        .entries
        .iter()
        .map(|entry| {
            let mut style = Style::default();
            if entry.file_type.is_dir() {
                style = style.fg(Color::Blue)
            } else if !sampler::can_load_file(&entry.path) {
                style = style.fg(Color::DarkGray)
            }
            ListItem::new(Span::styled(
                format!(" {}", entry.path.file_name().unwrap_or(""),),
                style,
            ))
        })
        .collect();
    let files = ListView::new(files).highlight_style(highlight_style);
    f.render_stateful_widget(files, sections[0], &mut view.files.state);

    let dir = shorten_path(&app.file_browser.dir, sections[1].width as usize - 8);
    let header = Paragraph::new(format!(" {}", dir)).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(BORDER_COLOR)),
    );
    f.render_widget(header, sections[1]);
}

fn animate<'a>(view: &View, states: Vec<Span<'a>>, state_dur: Duration) -> Span<'a> {
    let elapsed = view.frames as f64 / 30.0;
    let period = elapsed / state_dur.as_secs_f64();
    states[period.ceil() as usize % states.len()].clone()
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

fn render_outer_block(buffer: &mut Buffer, area: Rect, borders: Borders) -> Rect {
    let block = Block::default()
        .borders(borders)
        .border_style(Style::default().fg(BORDER_COLOR));
    let inner = block.inner(area);
    block.render(area, buffer);
    inner
}

fn highlight_style(view: &View, focus: Focus) -> Style {
    if view.focus == focus {
        Style::default().fg(Color::Black).bg(Color::Green)
    } else {
        Style::default()
    }
}
