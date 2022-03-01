use std::ops::Range;

use crate::app::{SharedState, ViewContext};
use crate::pattern::{Position, TrackView, MAX_PITCH};
use tui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, StatefulWidget, Widget},
};

#[derive(Clone, Default)]
pub struct EditorState {
    offset: usize,
}

pub struct Editor<'a> {
    ctx: ViewContext<'a>,
    cursor: Position,
    current_line: Option<usize>,
    lines_per_beat: usize,
    in_focus: bool,
    is_playing: bool,
}

impl<'a> Editor<'a> {
    pub fn new(cursor: Position, in_focus: bool, ctx: ViewContext<'a>) -> Self {
        let selected = ctx.selected_pattern_index();
        let active = ctx.active_pattern_index();
        let lines_per_beat = ctx.lines_per_beat() as usize;
        let is_playing = ctx.is_playing();
        let current_line = if selected == active {
            Some(ctx.current_line())
        } else {
            None
        };

        Self {
            ctx,
            current_line,
            cursor,
            lines_per_beat,
            in_focus,
            is_playing,
        }
    }

    fn render_track(
        &self,
        area: Rect,
        buf: &mut Buffer,
        track: &TrackView,
        index: usize,
        steps: &Range<usize>,
    ) {
        let width = COLUMN_WIDTH;

        // Draw track header
        let header = format!(" {} ", index);
        let padding = str::repeat(" ", width - header.len());
        let header = format!("{}{}", header, padding);
        let bg_color = Color::Indexed(250);
        buf.set_string(
            area.left(),
            area.top(),
            &header,
            Style::default().bg(bg_color).fg(Color::Black),
        );

        // Draw notes
        let mut y = area.top() + 1;
        for (line, note) in track.steps[steps.start..steps.end].iter().enumerate() {
            let line = line + steps.start;
            let base_style = self.get_base_style(line, false);
            let column = index * 2;

            let highlight = note.pitch.is_some() && self.is_playing;
            let pitch_style = self.get_input_style(line, column, highlight);
            let pitch = match note.pitch {
                Some(pitch) => &NOTE_NAMES[pitch as usize],
                None => "---",
            };

            let snd_style = self.get_input_style(line, column + 1, false);
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

    fn get_input_style(&self, line: usize, col: usize, active: bool) -> Style {
        if self.in_focus && self.cursor.line == line && self.cursor.column == col {
            Style::default().bg(Color::Green).fg(Color::Black)
        } else {
            self.get_base_style(line, active)
        }
    }

    fn get_base_style(&self, line: usize, active: bool) -> Style {
        if self.current_line.is_some() && self.current_line.unwrap() == line && active {
            Style::default().bg(Color::Indexed(239)).fg(Color::White)
        } else if line % self.lines_per_beat == 0 {
            Style::default().bg(Color::Indexed(236))
        } else {
            Style::default()
        }
    }
}

impl<'a> StatefulWidget for &Editor<'a> {
    type State = EditorState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let header_height = 1;
        let height = area.height as usize - header_height - 1;
        let pattern = self.ctx.selected_pattern();
        let mut last_line = state.offset + std::cmp::min(height, pattern.length);

        if last_line > pattern.length {
            // pattern length must have been changed so reset offset
            state.offset = 0;
            last_line = state.offset + std::cmp::min(height, pattern.length);
        }

        if self.cursor.line > last_line {
            last_line = self.cursor.line + 1;
            state.offset = last_line - height;
        } else if self.cursor.line < state.offset {
            state.offset = self.cursor.line;
            last_line = state.offset + height;
        }

        let left = area.left() + 1;

        let steps = state.offset..last_line;

        // Draw the step indicator next to the pattern grid
        for (i, step) in steps.clone().enumerate() {
            let style = if self.current_line.is_some() && self.current_line.unwrap() == step {
                Style::default().bg(Color::Blue).fg(Color::White)
            } else if step % self.lines_per_beat == 0 {
                Style::default().bg(Color::Indexed(236))
            } else {
                Style::default()
            };
            buf.set_string(
                left,
                area.top() + 1 + i as u16,
                format!("{:>3}", step),
                style,
            );
        }

        let mut x = area.x + STEP_COLUMN_WIDTH as u16;
        for (i, track) in pattern.iter_tracks().enumerate() {
            let mut borders = Borders::RIGHT;
            let mut width = COLUMN_WIDTH + 1;
            if i == 0 {
                // first track column also has a left border
                width += 1;
                borders |= Borders::LEFT;
            }

            let area = Rect {
                x,
                y: area.y,
                width: width as u16,
                height: (last_line - state.offset + 1) as u16,
            };
            x += width as u16;

            let block = Block::default().borders(borders);
            let inner = block.inner(area);
            block.render(area, buf);
            self.render_track(inner, buf, &track, i, &steps);
        }
    }
}

const COLUMN_WIDTH: usize = " C#4 05 ".len();
const STEP_COLUMN_WIDTH: usize = " 256 ".len();

lazy_static! {
    static ref NOTE_NAMES: Vec<String> = {
        let names = vec![
            "C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-",
        ];
        // 0 based octave notation instead of -2 based makes notes easier to read in the editor.
        (0..MAX_PITCH as usize)
            .map(|pitch| {
                let octave = pitch / 12;
                format!("{}{}", names[pitch % 12], octave)
            })
            .collect()
    };
}
