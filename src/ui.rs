pub mod editor;

pub use crate::input::{CommandState, EditMode, Input, InputQueue};
pub use crate::ui::editor::{Editor, EditorState};
use crate::{app::App, host::HostParam};
use tui::{
    backend::Backend,
    buffer::Buffer,
    layout::Rect,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget},
    Frame,
};

pub fn draw<B: Backend>(f: &mut Frame<B>, app: &mut App) {
    let screen = f.size();
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(screen.height - 2),
                Constraint::Length(1),
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(screen);

    let main = sections[0];
    let status = sections[1];
    let command = sections[2];

    let main_sections = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(80), Constraint::Percentage(20)].as_ref())
        .split(main);

    let editor_block = Block::default();
    let editor_area = editor_block.inner(main_sections[0]);
    f.render_widget(editor_block, main_sections[0]);

    let editor = Editor::new(&app);
    let mut edit_state = app.editor.clone();
    f.render_stateful_widget(&editor, editor_area, &mut edit_state);

    let command_line = CommandLine {
        edit_mode: app.mode,
    };
    f.render_stateful_widget(command_line, command, &mut app.command);

    let status_line = StatusLine::new(app);
    f.render_widget(&status_line, status);

    let sidebar_block = Block::default().borders(Borders::LEFT);
    let sidebar_area = sidebar_block.inner(main_sections[1]);
    f.render_widget(sidebar_block, main_sections[1]);
    render_sidebar(f, app, sidebar_area);
}

fn render_sidebar<B: Backend>(f: &mut Frame<B>, app: &mut App, area: Rect) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
            ]
            .as_ref(),
        )
        .split(area);

    // Instruments
    let instruments: Vec<ListItem> = app
        .instruments
        .iter()
        .enumerate()
        .map(|(i, instr)| {
            ListItem::new(Span::raw(format!(
                " {:0width$} {}",
                i,
                instr.name,
                width = 2
            )))
        })
        .collect();

    let instruments = List::new(instruments)
        .block(Block::default())
        .highlight_style(Style::default().fg(Color::White).bg(Color::Green));

    f.render_stateful_widget(instruments, sections[0], &mut app.instrument_list);

    // File Browser
    let file_sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(1),
                Constraint::Length(sections[1].height - 1),
            ]
            .as_ref(),
        )
        .split(sections[1]);
    let current_dir = format!(" {}", app.file_browser.current_dir());
    let header = Paragraph::new(current_dir).style(
        Style::default()
            .add_modifier(Modifier::REVERSED)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(header, file_sections[0]);
    let files: Vec<ListItem> = app
        .file_browser
        .iter()
        .map(|entry| {
            let file_name = entry
                .path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let file_name = format!(" {}", file_name);
            ListItem::new(Span::raw(file_name))
        })
        .collect();
    let files = List::new(files)
        .block(Block::default())
        .highlight_style(Style::default().fg(Color::White).bg(Color::Green));
    f.render_stateful_widget(files, file_sections[1], &mut app.files);

    // Parameters
    let column_width = sections[2].width as usize / 2;
    let track = app.editor.cursor.track;
    let params: Vec<ListItem> = app.instruments[track]
        .params
        .iter()
        .map(|(label, param)| {
            let label = format!("{}", label); // required to make padding work below
            ListItem::new(vec![Spans::from(vec![
                Span::raw(format!(" {:<width$}", label, width = column_width)),
                Span::raw(format!("{}", param)),
            ])])
        })
        .collect();

    let params = List::new(params)
        .block(Block::default().borders(Borders::TOP))
        .highlight_style(Style::default().fg(Color::White).bg(Color::Green));

    f.render_stateful_widget(params, sections[2], &mut app.params);
}

struct StatusLine {
    bpm: u16,
    lines_per_beat: u16,
    octave: u16,
}

impl StatusLine {
    fn new(app: &App) -> Self {
        Self {
            bpm: app.host_params.get(HostParam::Bpm),
            lines_per_beat: app.host_params.get(HostParam::LinesPerBeat),
            octave: app.host_params.get(HostParam::Octave),
        }
    }
}
impl Widget for &StatusLine {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let s = format!(
            " *Untitled*    BPM {}    LPB {}    Oct {}",
            self.bpm, self.lines_per_beat, self.octave
        );

        let offset = s.len();
        buf.set_string(
            area.left(),
            area.top(),
            s,
            Style::default().add_modifier(Modifier::REVERSED),
        );
        for x in offset..area.width as usize {
            buf.set_string(
                x as u16,
                area.top(),
                " ",
                Style::default().add_modifier(Modifier::REVERSED),
            );
        }
    }
}

pub struct CommandLine {
    edit_mode: EditMode,
}

impl StatefulWidget for CommandLine {
    type State = CommandState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if self.edit_mode == EditMode::Insert {
            buf.set_string(area.left(), area.top(), "-- INSERT --", Style::default());
        } else if state.buffer.len() > 0 {
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

    fn next(&mut self, num_items: usize) {
        let index = match self.selected() {
            Some(index) => {
                if index >= num_items - 1 {
                    0
                } else {
                    index + 1
                }
            }
            None => 0,
        };
        self.select(Some(index));
    }

    fn prev(&mut self, num_items: usize) {
        let index = match self.selected() {
            Some(index) => {
                if index == 0 {
                    num_items - 1
                } else {
                    index - 1
                }
            }
            None => 0,
        };
        self.select(Some(index));
    }
}
