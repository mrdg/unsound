use crate::pattern::{Position, TrackView};
use crate::{app::App, engine::EngineParam};

use tui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, StatefulWidget, Widget},
};

#[derive(Clone)]
pub struct EditorState {
    offset: usize,
}

impl EditorState {
    pub fn new() -> Self {
        Self { offset: 0 }
    }
}

pub struct Editor<'a> {
    app: &'a App,
    cursor: Position,
    lines_per_beat: usize,
}

impl<'a> Editor<'a> {
    pub fn new(app: &'a App) -> Self {
        Self {
            app,
            cursor: app.editor.cursor,
            lines_per_beat: app.engine_params.get(EngineParam::LinesPerBeat) as usize,
        }
    }

    fn render_track(&self, area: Rect, buf: &mut Buffer, track: &'a TrackView, index: usize) {
        let width = COLUMN_WIDTH;

        // Draw track header
        let header = format!(" {} ", index);
        let padding = str::repeat(" ", width - header.len());
        let header = format!("{}{}", header, padding);
        buf.set_string(
            area.left(),
            area.top(),
            &header,
            Style::default()
                .add_modifier(Modifier::REVERSED)
                .add_modifier(Modifier::BOLD),
        );

        // Draw notes
        let mut y = area.top() + 1;
        for (line, note) in track.steps.iter().enumerate() {
            let base_style = self.get_base_style(line);
            let column = index * 2;

            let pitch_style = self.get_input_style(line, column + 0);
            let pitch = match note.pitch {
                Some(pitch) => &NOTE_NAMES[pitch as usize],
                None => "---",
            };

            let snd_style = self.get_input_style(line, column + 1);
            let snd = match note.sound {
                Some(v) => format!("{:0width$}", v, width = 2),
                None => String::from("--"),
            };

            let spans = Spans::from(vec![
                Span::styled(" ", base_style),
                Span::styled(pitch, pitch_style),
                Span::styled(" ", base_style),
                Span::styled(snd, snd_style),
                Span::styled(" ", base_style),
            ]);

            buf.set_spans(area.left(), y, &spans, area.width);
            y += 1;
        }
    }

    fn get_input_style(&self, line: usize, col: usize) -> Style {
        if self.cursor.line == line && self.cursor.column == col {
            Style::default().bg(Color::Green).fg(Color::Black)
        } else {
            self.get_base_style(line)
        }
    }

    fn get_base_style(&self, line: usize) -> Style {
        if line == self.app.current_line {
            Style::default().bg(Color::Blue)
        } else if line % self.lines_per_beat == 0 {
            Style::default().bg(Color::DarkGray)
        } else {
            Style::default()
        }
    }
}

impl<'a> StatefulWidget for &Editor<'a> {
    type State = EditorState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let header_height = 1;
        let height = area.height as usize - header_height;
        let num_lines = self.app.editor.num_lines();
        let mut end_line = state.offset + std::cmp::min(height, num_lines);

        if self.cursor.line > end_line {
            end_line = self.cursor.line + 1;
            state.offset = end_line - height;
        } else if self.cursor.line < state.offset {
            state.offset = self.cursor.line;
            end_line = state.offset + height;
        }

        // Draw the step indicator next to the pattern grid
        // TODO: add border here
        let left = area.left() + 1;
        for (i, step) in (state.offset..end_line).enumerate() {
            let style = if step == self.app.current_line {
                Style::default().bg(Color::Blue).fg(Color::White)
            } else if step % self.lines_per_beat == 0 {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };
            buf.set_string(
                left,
                area.top() + 1 + i as u16,
                format!("{:0width$}", step, width = 2),
                style,
            );
            buf.set_string(left + 2, area.top() + 1 + i as u16, "|", Style::default());
        }

        let mut x = area.x + 3;
        for (i, track) in self.app.editor.iter_tracks().enumerate() {
            let area = Rect {
                x,
                y: area.y,
                width: COLUMN_WIDTH as u16 + 1, // add 1 for border
                height: (num_lines + 1) as u16,
            };
            x += area.width;
            let block = Block::default().borders(Borders::RIGHT);
            let inner = block.inner(area);
            block.render(area, buf);
            self.render_track(inner, buf, &track, i);
        }
    }
}

const COLUMN_WIDTH: usize = " C#4 05 ".len();

lazy_static! {
    static ref NOTE_NAMES: Vec<String> = {
        let names = vec![
            "C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-",
        ];
        // 0 based octave notation instead of -2 based makes notes easier to read in the editor.
        (0..127)
            .map(|pitch| {
                let octave = pitch / 12;
                format!("{}{}", names[pitch % 12], octave)
            })
            .collect()
    };
}
