use std::ops::Range;

use crate::app::{SharedState, TrackView, ViewContext};
use crate::pattern::{Position, INPUTS_PER_STEP, MAX_PITCH};
use tui::layout::{Alignment, Constraint, Direction, Layout};
use tui::widgets::Paragraph;
use tui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, StatefulWidget, Widget},
};

#[derive(Clone, Default)]
pub struct EditorState {
    line_offset: usize,
    track_offset: usize,
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
            let rms = self.ctx.rms(track.index);
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

            let spans = Spans::from(vec![
                Span::styled(&meter_symbol, Style::default().fg(left_color)),
                Span::raw(" "),
                Span::styled(&meter_symbol, Style::default().fg(right_color)),
            ]);
            buf.set_spans(meter.x, meter.y + i, &spans, meter_width + 1);

            db -= 6;
        }

        // Volume control
        let volume_area = Rect {
            x: area.x,
            y: meter.y + meter.height,
            width: area.width,
            height: 2,
        };

        let volume = format!("{:.2}", track.volume);
        let volume = Paragraph::new(volume)
            .alignment(tui::layout::Alignment::Center)
            .block(Block::default().borders(Borders::TOP));
        volume.render(volume_area, buf);

        let button_area = Rect {
            x: area.x,
            y: meter.y + meter.height + 2,
            width: area.width,
            height: 2,
        };

        if track.is_master {
            let block = Block::default().borders(Borders::TOP);
            block.render(button_area, buf);
            return;
        }

        let button_style = if track.muted {
            Style::default().bg(Color::DarkGray)
        } else {
            Style::default().bg(Color::Yellow).fg(Color::Black)
        };

        let button = Span::styled(format!(" {} ", track.index), button_style);
        let button = Paragraph::new(button)
            .alignment(tui::layout::Alignment::Center)
            .block(Block::default().borders(Borders::TOP));
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
        steps: &Range<usize>,
    ) {
        let mut y = area.top() + 1;
        for (line, note) in track.steps(steps).iter().enumerate() {
            let line = line + steps.start;
            let column = track.index * INPUTS_PER_STEP;

            let pitch = match note.pitch {
                Some(pitch) => &NOTE_NAMES[pitch as usize],
                None => "---",
            };

            let snd = match note.sound {
                Some(v) => format!("{:0width$}", v, width = 2),
                None => String::from("--"),
            };

            let effects: Vec<(String, String)> = [note.effect1, note.effect2]
                .iter()
                .map(|effect| match effect {
                    Some(effect) => {
                        let desc = effect.desc();
                        (
                            desc.effect_type,
                            desc.value.unwrap_or_else(|| "---".to_string()),
                        )
                    }
                    None => ("-".to_string(), "---".to_string()),
                })
                .collect();

            let line_style = if line % self.lines_per_beat == 0 {
                Style::default().bg(Color::Indexed(236))
            } else {
                Style::default()
            };
            let input_style = |offset: usize| {
                if self.in_focus
                    && self.cursor.line == line
                    && self.cursor.column == column + offset
                {
                    Style::default().bg(Color::Green).fg(Color::Black)
                } else if self.current_line.unwrap_or(usize::MAX) == line
                    && offset == 0
                    && note.pitch.is_some()
                    && self.is_playing
                {
                    // Pitch input is highlighted when it's the currently active note
                    Style::default().bg(Color::Indexed(239)).fg(Color::White)
                } else {
                    line_style
                }
            };

            let spans = Spans::from(vec![
                Span::styled(" ", line_style),
                Span::styled(pitch, input_style(0)),
                Span::styled(" ", line_style),
                Span::styled(snd, input_style(1)),
                Span::styled(" ", line_style),
                Span::styled(&effects[0].0, input_style(2)),
                Span::styled(&effects[0].1, input_style(3)),
                Span::styled(" ", line_style),
                Span::styled(&effects[1].0, input_style(4)),
                Span::styled(&effects[1].1, input_style(5)),
                Span::styled(" ", line_style),
            ]);

            buf.set_spans(area.left(), y, &spans, area.width);
            y += 1;
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

        if self.cursor.line > last_line {
            last_line = self.cursor.line + 1;
            state.line_offset = last_line - height;
        } else if self.cursor.line < state.line_offset {
            state.line_offset = self.cursor.line;
            last_line = state.line_offset + height;
        }

        let pattern_width = pattern_area.width - STEPS_WIDTH;
        let num_tracks = ((pattern_width - MASTER_TRACK_WIDTH) / TRACK_WIDTH) as usize;

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
            let block = Block::default().borders(borders);
            let inner = block.inner(area);
            block.render(area, buf);
            self.render_track_header(inner, buf, track);
            if !track.is_master {
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
            let block = Block::default().borders(borders);
            let inner = block.inner(area);
            block.render(area, buf);
            self.render_mixer_controls(track, inner, buf);
        };

        for (_, track) in self
            .ctx
            .iter_tracks()
            .filter(|t| !t.is_master)
            .enumerate()
            .filter(|(i, _)| *i >= state.track_offset && *i < state.track_offset + num_tracks)
        {
            render_track(x, TRACK_WIDTH, &track);
            x += TRACK_WIDTH;
        }

        // Master track sticks to the right of the editor area
        let master_track = self.ctx.master_track();
        let x = area.x + (area.width - MASTER_TRACK_WIDTH);
        render_track(x, MASTER_TRACK_WIDTH, &master_track);
    }
}

const TRACK_WIDTH: u16 = "| C#4 05 v 20 R-10 |".len() as u16;
const MASTER_TRACK_WIDTH: u16 = 12;
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
