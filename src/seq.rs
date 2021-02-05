use crate::app::{HostParam, HostState};
use crate::SAMPLE_RATE;

const MAX_INSTRUMENTS: usize = 32;
const MAX_PATTERN_LENGTH: usize = 512;
const COLUMNS_PER_TRACK: usize = 8;
const COLUMNS_PER_LINE: usize = COLUMNS_PER_TRACK * MAX_INSTRUMENTS;

#[derive(Clone, Copy, Debug)]
pub enum Event {
    Empty,
    NoteOn { pitch: i32 },
    NoteOff { pitch: i32 },
}

#[derive(Debug)]
pub struct Block {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone)]
pub struct Pattern {
    pub events: Vec<Event>,
    pub num_lines: usize,
    pub num_tracks: usize,
}

#[derive(Clone, Copy)]
pub struct Slot {
    pub line: usize,
    pub track: usize,
    pub lane: usize,
}

impl Pattern {
    pub fn new() -> Self {
        Self {
            events: vec![Event::Empty; MAX_INSTRUMENTS * COLUMNS_PER_TRACK * MAX_PATTERN_LENGTH],
            num_lines: 32,
            num_tracks: 0,
        }
    }

    pub fn with_pitches(track_pitches: Vec<Vec<i32>>) -> Self {
        let mut pattern = Self::new();
        for (track, pitches) in track_pitches.iter().enumerate() {
            for (line, &pitch) in pitches.iter().enumerate() {
                if pitch == 0 {
                    continue;
                }
                pattern.add_event(
                    Event::NoteOn { pitch: pitch },
                    Slot {
                        line,
                        track,
                        lane: 0,
                    },
                );
            }
        }
        pattern
    }

    pub fn add_event(&mut self, event: Event, slot: Slot) {
        let i = COLUMNS_PER_LINE * slot.line + slot.track * COLUMNS_PER_TRACK;
        self.events[i] = event;
    }

    pub fn event_at(&self, line: usize, track: usize) -> Event {
        self.events[COLUMNS_PER_LINE * line + track * COLUMNS_PER_TRACK]
    }
}

pub struct Sequencer {
    samples_to_next_pulse: usize,
    current_pulse: i64,
}

impl Sequencer {
    pub fn new() -> Sequencer {
        Sequencer {
            samples_to_next_pulse: 0,
            current_pulse: 0,
        }
    }

    pub fn next_block(
        &mut self,
        block: &mut Block,
        num_frames: usize,
        state: &mut HostState,
    ) -> bool {
        if !state.is_playing {
            if block.end == num_frames {
                return false;
            }
            block.end = num_frames;
            return true;
        }

        self.samples_to_next_pulse -= block.end - block.start;
        if block.end == num_frames {
            return false;
        }
        if block.end != 0 {
            block.start = block.end;
        }

        let pattern = &state.current_pattern;

        if self.samples_to_next_pulse == 0 {
            let line = self.current_pulse % pattern.num_lines as i64;
            let start = line as usize * COLUMNS_PER_LINE;
            let end = start + COLUMNS_PER_LINE;
            for column in start..end {
                let event = &pattern.events[column];
                if let Event::Empty = event {
                    continue;
                }
                let index = (column - start) / COLUMNS_PER_TRACK;
                let lane = (column - start) % COLUMNS_PER_TRACK;
                if let Some(Some(instrument)) = state.track_mapping.get_mut(index) {
                    instrument.send_event(lane, event);
                }
            }
            self.current_pulse += 1;
            let bpm = state.params.get(&HostParam::Bpm).unwrap();
            let lines_per_beat = state.params.get(&HostParam::LinesPerBeat).unwrap();
            let num_samples =
                (SAMPLE_RATE * 60.) / (*lines_per_beat as usize * *bpm as usize) as f64;
            self.samples_to_next_pulse = num_samples.round() as usize;
            state.set_current_line(line as usize);
        }

        block.end = block.start + self.samples_to_next_pulse;
        if block.end > num_frames {
            block.end = num_frames;
        }
        true
    }
}
