use crate::app::ClientState;
use crate::seq::{Event, Pattern, Slot};
use termion::event::Key;

use tui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::StatefulWidget,
};

pub struct EditorState {
    pub cursor: Slot,
    pub pending_key: Option<Key>,
    offset: usize,
}

impl EditorState {
    pub fn new() -> Self {
        Self {
            cursor: Slot {
                track: 0,
                line: 0,
                lane: 0,
            },
            pending_key: None,
            offset: 0,
        }
    }
}

pub struct Editor<'a> {
    pattern: &'a Pattern,
    note_names: Vec<String>,
    num_tracks: usize,
    lines_per_beat: usize,
    current_line: usize,
}

impl<'a> Editor<'a> {
    pub fn new(state: &'a ClientState) -> Self {
        Self {
            pattern: &state.current_pattern,
            note_names: note_names(),
            num_tracks: state.instruments.len(),
            lines_per_beat: state.lines_per_beat,
            current_line: state.current_line,
        }
    }
}

impl<'a> StatefulWidget for &Editor<'a> {
    type State = EditorState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let header_height = 1;
        let height = area.height as usize - header_height;
        let mut end_line = state.offset + std::cmp::min(height, self.pattern.num_lines);

        if state.cursor.line > end_line {
            end_line = state.cursor.line + 1;
            state.offset = end_line - height;
        } else if state.cursor.line < state.offset {
            state.offset = state.cursor.line;
            end_line = state.offset + height;
        }

        // Draw the step indicator next to the pattern grid
        let mut left = area.left() + 1;
        for (i, step) in (state.offset..end_line).enumerate() {
            let style = if step == self.current_line {
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
        left += 3;

        // Draw the track headers
        let col_width = 10;
        for n in 0..self.num_tracks {
            let header = format!(" Track {} |", n);
            buf.set_string(
                left + (n * col_width) as u16,
                area.top(),
                header,
                Style::default()
                    .add_modifier(Modifier::REVERSED)
                    .add_modifier(Modifier::BOLD),
            );
        }

        // Draw the notes
        for (i, line) in (state.offset..end_line).enumerate() {
            let y = area.top() + (i + 1) as u16;
            for column in 0..self.num_tracks {
                let event = self.pattern.event_at(line, column);
                let text = match event {
                    Event::NoteOn { pitch } => format!(" {} --  ", self.note_names[pitch as usize]),
                    Event::NoteOff { pitch: _ } => format!(" OFF --  "),
                    Event::Empty => format!(" --- --  "),
                };

                let style = if state.cursor.line == line && state.cursor.track == column {
                    Style::default().bg(Color::Green)
                } else if line == self.current_line {
                    Style::default().bg(Color::Blue)
                } else if line % self.lines_per_beat == 0 {
                    Style::default().bg(Color::DarkGray)
                } else {
                    Style::default()
                };
                let x = left + (column * col_width) as u16;
                buf.set_string(x, y, text, style);
                buf.set_string(x + (col_width - 1) as u16, y, "|", Style::default());
            }
        }
    }
}

fn note_names() -> Vec<String> {
    let names = vec![
        "C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-",
    ];
    // 0 based octave notation instead of -2 based makes notes easier to read in the editor.
    (0..127)
        .map(|pitch| {
            let octave = pitch / 12;
            let name = format!("{}{}", names[pitch % 12], octave);
            name
        })
        .collect()
}
