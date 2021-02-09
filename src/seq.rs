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
