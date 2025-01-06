use std::ops::Range;

use crate::app::{App, Track};
use crate::engine::TrackParams;
use crate::pattern::{Position, INPUTS_PER_STEP, MAX_PITCH};
use crate::view::{render_outer_block, Focus, View, BORDER_COLOR};

use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::widgets::Paragraph;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Widget},
};

const TRACK_WIDTH: u16 = "| C#4 05 v 20 R-10 |".len() as u16;
const BUS_TRACK_WIDTH: u16 = 12;
const STEPS_WIDTH: u16 = " 256 ".len() as u16;

#[derive(Clone, Default)]
pub struct EditorState {
    pub cursor: Position,
    line_offset: usize,
    track_offset: usize,
}

pub fn render(app: &App, view: &mut View, area: Rect, buf: &mut Buffer) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(75), Constraint::Percentage(25)].as_ref())
        .split(area);

    let pattern_area = sections[0];
    let mixer_area = sections[1];

    let header_height = 1;
    let height = pattern_area.height as usize - header_height - 1;

    let pattern = app.state.selected_pattern();
    let mut last_line = view.editor.line_offset + std::cmp::min(height, pattern.len());

    let pattern_size = pattern.size();
    let cursor = &mut view.editor.cursor;
    cursor.line = usize::min(pattern_size.lines - 1, cursor.line);
    cursor.column = usize::min(pattern_size.columns - 1, cursor.column);

    if last_line > pattern.len() {
        // pattern length must have been changed so reset offset
        view.editor.line_offset = 0;
        last_line = view.editor.line_offset + std::cmp::min(height, pattern.len());
    }

    if view.editor.cursor.line >= last_line {
        last_line = view.editor.cursor.line + 1;
        view.editor.line_offset = last_line - height;
    } else if view.editor.cursor.line < view.editor.line_offset {
        view.editor.line_offset = view.editor.cursor.line;
        last_line = view.editor.line_offset + height;
    }

    let pattern_width = pattern_area.width - STEPS_WIDTH;
    let num_tracks = ((pattern_width - BUS_TRACK_WIDTH) / TRACK_WIDTH) as usize;

    let selected_track = view.editor.cursor.track();
    if selected_track >= view.editor.track_offset + num_tracks {
        view.editor.track_offset = selected_track + 1 - num_tracks;
    } else if selected_track < view.editor.track_offset {
        view.editor.track_offset = selected_track;
    }

    let left = area.left() + 1;
    let steps = view.editor.line_offset..last_line;

    // Draw the step indicator next to the pattern grid
    let style = Style::default().fg(Color::Indexed(241));
    buf.set_string(left, area.top(), format!("{:>3}", pattern.len()), style);
    for (i, step) in steps.clone().enumerate() {
        let style = if is_current_line(app, step) {
            Style::default().bg(Color::Blue).fg(Color::White)
        } else if step % app.state.lines_per_beat as usize == 0 {
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

    let mut x = area.x + STEPS_WIDTH;
    let mut render_track = |x: u16, width: u16, track: &Track, idx: usize| {
        let mut borders = Borders::RIGHT | Borders::BOTTOM | Borders::LEFT;

        // Draw pattern
        let area = Rect {
            x,
            y: area.y,
            width,
            height: (last_line - view.editor.line_offset + 2) as u16,
        };

        let inner = render_outer_block(buf, area, borders);
        let track_name = if let Some(name) = &track.name {
            format!(" {}", name)
        } else {
            format!(" {}", idx)
        };
        let bg_color = Color::Indexed(250);
        let header = Paragraph::new(track_name)
            .alignment(Alignment::Left)
            .style(Style::default().bg(bg_color).fg(Color::Black));

        let header_area = Rect { height: 1, ..inner };
        header.render(header_area, buf);

        if !track.is_bus() {
            render_track_steps(app, view, buf, inner, idx, &steps);
        }

        // Draw mixer channel
        let area = Rect {
            x,
            y: mixer_area.y,
            width,
            height: mixer_area.height,
        };

        borders |= Borders::TOP;
        let inner = render_outer_block(buf, area, borders);
        render_mixer_controls(app, track, buf, inner, idx);
    };

    for (idx, track) in app.state.tracks.iter().enumerate() {
        if track.is_bus() {
            continue;
        }
        if idx >= view.editor.track_offset && idx < view.editor.track_offset + num_tracks {
            render_track(x, TRACK_WIDTH, track, idx);
            x += TRACK_WIDTH;
        }
    }

    // Master track sticks to the right of the editor area
    let master_track = app.state.tracks.last().unwrap();
    let master_idx = app.state.tracks.len() - 1;
    render_track(
        area.x + (area.width - BUS_TRACK_WIDTH),
        BUS_TRACK_WIDTH,
        master_track,
        master_idx,
    );
}

fn render_mixer_controls(app: &App, track: &Track, buf: &mut Buffer, area: Rect, idx: usize) {
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

    let volume = app.params(track.device_id).get_param(TrackParams::VOLUME);
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(BORDER_COLOR));
    let volume = Paragraph::new(volume.as_string())
        .alignment(Alignment::Center)
        .block(block);
    volume.render(volume_area, buf);

    let button_area = Rect {
        x: area.x,
        y: meter.y + meter.height + 2,
        width: area.width,
        height: 2,
    };

    if track.is_bus() {
        render_outer_block(buf, button_area, Borders::TOP);
        return;
    }

    let muted = app.params(track.device_id).get_param(TrackParams::MUTE);
    let button_style = if muted.as_bool() {
        Style::default().bg(Color::DarkGray)
    } else {
        Style::default().bg(Color::Yellow).fg(Color::Black)
    };

    let button = Span::styled(format!(" {} ", idx), button_style);
    let button = Paragraph::new(button).alignment(Alignment::Center).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(BORDER_COLOR)),
    );
    button.render(button_area, buf);
}

fn render_track_steps(
    app: &App,
    view: &View,
    buf: &mut Buffer,
    area: Rect,
    idx: usize,
    step_range: &Range<usize>,
) {
    let mut y = area.top() + 1;
    for (line, step) in app.state.pattern_steps(idx, step_range).iter().enumerate() {
        let line = line + step_range.start;
        let column = idx * INPUTS_PER_STEP;

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
            .map(|c| (c as char).to_string())
            .unwrap_or_else(|| "-".into());
        let fx_val1 = step
            .effect_val(0)
            .map(|c| format!("{:3}", c))
            .unwrap_or_else(|| "---".into());
        let fx_cmd2 = step
            .effect_cmd(1)
            .map(|c| (c as char).to_string())
            .unwrap_or_else(|| "-".into());
        let fx_val2 = step
            .effect_val(1)
            .map(|c| format!("{:3}", c))
            .unwrap_or_else(|| "---".into());

        let line_style = if line % app.state.lines_per_beat as usize == 0 {
            Style::default().bg(Color::Indexed(236))
        } else {
            Style::default()
        };
        let input_style = |offset: usize| {
            let selected = view
                .selection
                .as_ref()
                .map(|s| s.contains(line, column + offset))
                .unwrap_or(false);

            if matches!(view.focus, Focus::Editor)
                && view.editor.cursor.line == line
                && view.editor.cursor.column == column + offset
            {
                Style::default().bg(Color::Green).fg(Color::Black)
            } else if selected {
                Style::default().bg(Color::Rgb(65, 79, 139))
            } else if is_current_line(app, line)
                && offset == 0
                && step.pitch().is_some()
                && app.state.is_playing
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

fn is_current_line(app: &App, line: usize) -> bool {
    if app.state.selected_pattern != app.engine_state.current_pattern {
        false
    } else {
        app.engine_state.current_line() == line
    }
}

lazy_static! {
    static ref NOTE_NAMES: Vec<String> = {
        let names = [
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
