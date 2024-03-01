use std::ops::Range;

use crate::pattern::{Position, Selection, INPUTS_PER_STEP, MAX_PITCH};
use crate::view::context::{TrackView, ViewContext};
use crate::view::Focus;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::widgets::Paragraph;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, StatefulWidget, Widget},
};

use super::BORDER_COLOR;

#[derive(Clone, Default)]
pub struct EditorState {
    line_offset: usize,
    track_offset: usize,
}

pub struct Editor<'a> {
    ctx: ViewContext<'a>,
    cursor: Position,
    focus: Focus,
    selection: &'a Option<Selection>,
}

impl<'a> Editor<'a> {
    pub fn new(
        cursor: Position,
        focus: Focus,
        selection: &'a Option<Selection>,
        ctx: ViewContext<'a>,
    ) -> Self {
        Self {
            ctx,
            cursor,
            focus,
            selection,
        }
    }

    fn render_mixer_controls(&self, track: &TrackView, area: Rect, buf: &mut Buffer) {
        let mut meter_width = 2;
        if area.width % 2 != 0 {
            meter_width += 1;
        }
        let offset = (area.width - meter_width) / 2;

        // VU meter
        let meter = Rect {
            x: area.x + offset,
            y: area.y,
            width: meter_width,
            height: area.height - 4,
        };

        let mut db = 0;
        for i in 0..meter.height {
            let rms = track.rms();
            let meter_color = |value: f32| {
                let db = db as f32;
                if value > db {
                    if value < db + 2.0 {
                        Color::Indexed(34)
                    } else if value < db + 4.0 {
                        Color::Indexed(40)
                    } else {
                        Color::Indexed(46)
                    }
                } else {
                    Color::Gray
                }
            };
            let left_color = meter_color(rms.0);
            let right_color = meter_color(rms.1);

            let channel_width = meter_width / 2;
            let meter_symbol = "▇".repeat(channel_width.into());

            let spans = Line::from(vec![
                Span::styled(&meter_symbol, Style::default().fg(left_color)),
                Span::raw(" "),
                Span::styled(&meter_symbol, Style::default().fg(right_color)),
            ]);
            buf.set_line(meter.x, meter.y + i, &spans, meter_width + 1);

            db -= 6;
        }

        // Volume control
        let volume_area = Rect {
            x: area.x,
            y: meter.y + meter.height,
            width: area.width,
            height: 2,
        };

        let volume = format!("{:.2}", track.volume());
        let volume = Paragraph::new(volume)
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(BORDER_COLOR)),
            );
        volume.render(volume_area, buf);

        let button_area = Rect {
            x: area.x,
            y: meter.y + meter.height + 2,
            width: area.width,
            height: 2,
        };

        if track.is_bus() {
            let block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(BORDER_COLOR));
            block.render(button_area, buf);
            return;
        }

        let button_style = if track.is_muted() {
            Style::default().bg(Color::DarkGray)
        } else {
            Style::default().bg(Color::Yellow).fg(Color::Black)
        };

        let button = Span::styled(format!(" {} ", track.index), button_style);
        let button = Paragraph::new(button)
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(BORDER_COLOR)),
            );
        button.render(button_area, buf);
    }

    fn render_track_header(&self, area: Rect, buf: &mut Buffer, track: &TrackView) {
        let track_name = format!(" {}", track.name());
        let bg_color = Color::Indexed(250);
        let header = Paragraph::new(track_name)
            .alignment(Alignment::Left)
            .style(Style::default().bg(bg_color).fg(Color::Black));

        let header_area = Rect { height: 1, ..area };
        header.render(header_area, buf);
    }

    fn render_track_steps(
        &self,
        area: Rect,
        buf: &mut Buffer,
        track: &TrackView,
        step_range: &Range<usize>,
    ) {
        let mut y = area.top() + 1;
        for (line, step) in self
            .ctx
            .pattern_steps(track.index, step_range)
            .iter()
            .enumerate()
        {
            let line = line + step_range.start;
            let column = track.index * INPUTS_PER_STEP;

            let pitch = match step.pitch() {
                Some(pitch) => &NOTE_NAMES[pitch as usize],
                None => "---",
            };

            let snd = match step.instrument() {
                Some(v) => format!("{:0width$}", v, width = 2),
                None => String::from("--"),
            };

            let fx_cmd1 = step
                .effect_cmd(0)
                .map(|c| (c as u8 as char).to_string())
                .unwrap_or_else(|| "-".into());
            let fx_val1 = step
                .effect_val(0)
                .map(|c| format!("{:3}", c))
                .unwrap_or_else(|| "---".into());
            let fx_cmd2 = step
                .effect_cmd(1)
                .map(|c| (c as u8 as char).to_string())
                .unwrap_or_else(|| "-".into());
            let fx_val2 = step
                .effect_val(1)
                .map(|c| format!("{:3}", c))
                .unwrap_or_else(|| "---".into());

            let line_style = if line % self.ctx.lines_per_beat() as usize == 0 {
                Style::default().bg(Color::Indexed(236))
            } else {
                Style::default()
            };
            let input_style = |offset: usize| {
                let selected = self
                    .selection
                    .as_ref()
                    .map(|s| s.contains(line, column + offset))
                    .unwrap_or(false);

                if matches!(self.focus, Focus::Editor)
                    && self.cursor.line == line
                    && self.cursor.column == column + offset
                {
                    Style::default().bg(Color::Green).fg(Color::Black)
                } else if selected {
                    Style::default().bg(Color::Rgb(65, 79, 139))
                } else if self.is_current_line(line)
                    && offset == 0
                    && step.pitch().is_some()
                    && self.ctx.is_playing()
                {
                    // Pitch input is highlighted when it's the currently active note
                    Style::default().bg(Color::Indexed(239)).fg(Color::White)
                } else {
                    line_style
                }
            };

            let spans = Line::from(vec![
                Span::styled(" ", line_style),
                Span::styled(pitch, input_style(0)),
                Span::styled(" ", line_style),
                Span::styled(snd, input_style(1)),
                Span::styled(" ", line_style),
                Span::styled(fx_cmd1, input_style(2)),
                Span::styled(fx_val1, input_style(3)),
                Span::styled(" ", line_style),
                Span::styled(fx_cmd2, input_style(4)),
                Span::styled(fx_val2, input_style(5)),
                Span::styled(" ", line_style),
            ]);

            buf.set_line(area.left(), y, &spans, area.width);
            y += 1;
        }
    }

    fn is_current_line(&self, line: usize) -> bool {
        if self.ctx.selected_pattern_index() != self.ctx.active_pattern_index() {
            false
        } else {
            self.ctx.current_line() == line
        }
    }
}

impl<'a> StatefulWidget for &Editor<'a> {
    type State = EditorState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(75), Constraint::Percentage(25)].as_ref())
            .split(area);

        let pattern_area = sections[0];
        let mixer_area = sections[1];

        let header_height = 1;
        let height = pattern_area.height as usize - header_height - 1;
        let pattern = self.ctx.selected_pattern();
        let mut last_line = state.line_offset + std::cmp::min(height, pattern.len());

        if last_line > pattern.len() {
            // pattern length must have been changed so reset offset
            state.line_offset = 0;
            last_line = state.line_offset + std::cmp::min(height, pattern.len());
        }

        if self.cursor.line >= last_line {
            last_line = self.cursor.line + 1;
            state.line_offset = last_line - height;
        } else if self.cursor.line < state.line_offset {
            state.line_offset = self.cursor.line;
            last_line = state.line_offset + height;
        }

        let pattern_width = pattern_area.width - STEPS_WIDTH;
        let num_tracks = ((pattern_width - BUS_TRACK_WIDTH) / TRACK_WIDTH) as usize;

        let selected_track = self.cursor.track();
        if selected_track >= state.track_offset + num_tracks {
            state.track_offset = selected_track + 1 - num_tracks;
        } else if selected_track < state.track_offset {
            state.track_offset = selected_track;
        }

        let left = area.left() + 1;
        let steps = state.line_offset..last_line;

        // Draw the step indicator next to the pattern grid
        let style = Style::default().fg(Color::Indexed(241));
        buf.set_string(left, area.top(), format!("{:>3}", pattern.len()), style);
        for (i, step) in steps.clone().enumerate() {
            let style = if self.is_current_line(step) {
                Style::default().bg(Color::Blue).fg(Color::White)
            } else if step % self.ctx.lines_per_beat() as usize == 0 {
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

        let mut x = area.x + STEPS_WIDTH as u16;
        let mut render_track = |x: u16, width: u16, track: &TrackView| {
            let mut borders = Borders::RIGHT | Borders::BOTTOM | Borders::LEFT;

            // Draw pattern
            let area = Rect {
                x,
                y: area.y,
                width,
                height: (last_line - state.line_offset + 2) as u16,
            };
            let block = Block::default()
                .borders(borders)
                .border_style(Style::default().fg(BORDER_COLOR));
            let inner = block.inner(area);
            block.render(area, buf);
            self.render_track_header(inner, buf, track);
            if !track.is_bus() {
                self.render_track_steps(inner, buf, track, &steps);
            }

            // Draw mixer channel
            let area = Rect {
                x,
                y: mixer_area.y,
                width,
                height: mixer_area.height,
            };
            borders |= Borders::TOP;
            let block = Block::default()
                .borders(borders)
                .border_style(Style::default().fg(BORDER_COLOR));
            let inner = block.inner(area);
            block.render(area, buf);
            self.render_mixer_controls(track, inner, buf);
        };

        for track in self.ctx.iter_tracks().filter(|t| {
            !t.is_bus()
                && t.index >= state.track_offset
                && t.index < state.track_offset + num_tracks
        }) {
            render_track(x, TRACK_WIDTH, &track);
            x += TRACK_WIDTH;
        }

        // Master track sticks to the right of the editor area
        let master_track = self.ctx.master_bus();
        let x = area.x + (area.width - BUS_TRACK_WIDTH);
        render_track(x, BUS_TRACK_WIDTH, &master_track);
    }
}

const TRACK_WIDTH: u16 = "| C#4 05 v 20 R-10 |".len() as u16;
const BUS_TRACK_WIDTH: u16 = 12;
const STEPS_WIDTH: u16 = " 256 ".len() as u16;

lazy_static! {
    static ref NOTE_NAMES: Vec<String> = {
        let names = vec![
            "C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-",
        ];
        // 0 based octave notation instead of -2 based makes notes easier to read in the editor.
        let mut notes: Vec<String> = (0..MAX_PITCH as usize)
            .map(|pitch| {
                let octave = pitch / 12;
                format!("{}{}", names[pitch % 12], octave)
            })
            .collect();

        notes.push("OFF".to_string());
        notes
    };
}
