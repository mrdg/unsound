mod editor;
mod state;

use crate::app::ClientState;
use crate::ui::editor::Editor;
use crate::ui::state::{CommandState, EditMode, ViewState};
use anyhow::Result;
use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use termion::{
    event::Key, input::MouseTerminal, input::TermRead, raw::IntoRawMode, screen::AlternateScreen,
};
use tui::{
    backend::{Backend, TermionBackend},
    buffer::Buffer,
    layout::Rect,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, List, ListItem, ListState, StatefulWidget, Widget},
    Frame, Terminal,
};

pub struct Ui {
    state: ViewState,
    input_events: mpsc::Receiver<Input>,
}

enum Input {
    Key(Key),
    Tick,
}

impl Ui {
    pub fn new(state: ClientState) -> Result<Self> {
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

        Ok(Self {
            input_events: receiver,
            state: ViewState::new(state),
        })
    }

    pub fn run(mut self) -> Result<()> {
        let stdout = io::stdout().into_raw_mode()?;
        let stdout = MouseTerminal::from(stdout);
        let stdout = AlternateScreen::from(stdout);
        let backend = TermionBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        loop {
            self.state.app.apply_updates();
            if self.state.app.should_stop {
                return Ok(());
            }
            terminal.draw(|f| {
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

                let editor = Editor::new(&self.state.app);
                f.render_stateful_widget(&editor, editor_area, &mut self.state.editor);

                let command_line = CommandLine {
                    edit_mode: self.state.mode,
                };
                f.render_stateful_widget(command_line, command, &mut self.state.command);

                let status_line = StatusLine::new(&self.state.app);
                f.render_widget(&status_line, status);

                let sidebar_block = Block::default().borders(Borders::LEFT);
                let sidebar_area = sidebar_block.inner(main_sections[1]);
                f.render_widget(sidebar_block, main_sections[1]);
                self.render_sidebar(f, sidebar_area);
            })?;

            match self.input_events.recv()? {
                Input::Key(key) => self.state.handle_input(key)?,
                Input::Tick => {}
            }
        }
    }

    fn render_sidebar<B: Backend>(&mut self, f: &mut Frame<B>, area: Rect) {
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
            .split(area);

        let instruments: Vec<ListItem> = self
            .state
            .app
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

        f.render_stateful_widget(instruments, sections[0], &mut self.state.instruments);

        let column_width = 3 * (sections[1].width as usize / 4);

        let track = self.state.editor.cursor.track;
        let params: Vec<ListItem> = self.state.app.instruments[track]
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

        f.render_stateful_widget(params, sections[1], &mut self.state.params);
    }
}

struct StatusLine {
    bpm: u16,
    lines_per_beat: usize,
    octave: i32,
}

impl StatusLine {
    fn new(state: &ClientState) -> Self {
        Self {
            bpm: state.bpm,
            lines_per_beat: state.lines_per_beat,
            octave: state.octave,
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

trait ListCursorExt {
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
