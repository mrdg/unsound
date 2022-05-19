use crate::engine::INSTRUMENT_TRACKS;
use crate::sampler::ROOT_PITCH;

pub const INPUTS_PER_STEP: usize = 2;
pub const MAX_PITCH: u8 = 109;
pub const NOTE_OFF: u8 = MAX_PITCH;
pub const MAX_PATTERNS: usize = 256;

const MAX_PATTERN_LENGTH: usize = 512;

pub enum InputType {
    Pitch,
    Sound,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

impl Position {
    pub fn track(&self) -> usize {
        self.column / INPUTS_PER_STEP
    }

    pub fn clamp(&mut self, pattern_size: (usize, usize)) {
        self.line = usize::min(pattern_size.0 - 1, self.line);
        self.column = usize::min(pattern_size.1 - 1, self.column);
    }

    pub fn input_type(&self) -> InputType {
        match self.column % INPUTS_PER_STEP {
            0 => InputType::Pitch,
            1 => InputType::Sound,
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Pattern {
    pub length: usize,
    pub tracks: Vec<Track>,
}

impl Pattern {
    pub fn new() -> Self {
        let mut tracks = Vec::with_capacity(INSTRUMENT_TRACKS);
        for _ in 0..tracks.capacity() {
            tracks.push(Track {
                steps: vec![Step::default(); MAX_PATTERN_LENGTH],
            })
        }
        Self { length: 16, tracks }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.length, self.tracks.len() * INPUTS_PER_STEP)
    }

    pub fn set_pitch(&mut self, pos: Position, pitch: u8) {
        if pitch <= MAX_PITCH {
            let v = self.input(pos);
            *v = Some(pitch);
        }
    }

    pub fn set_note_off(&mut self, pos: Position) {
        let v = self.input(pos);
        *v = Some(NOTE_OFF);
    }

    pub fn set_sound(&mut self, pos: Position, num: i32) {
        let s = self.input(pos).get_or_insert(0);
        let i = *s as i32;
        *s = ((i * 10 + num) % 100) as u8;
    }

    pub fn delete(&mut self, pos: Position) {
        let v = self.input(pos);
        *v = None;
    }

    pub fn inc(&mut self, pos: Position, step_size: StepSize) {
        let attrs = input_attrs(pos.column % INPUTS_PER_STEP);
        let add = attrs.step_sizes[step_size as usize];
        let input = self.input(pos).get_or_insert(attrs.default);
        if let Some(new) = input.checked_add(add) {
            if new < attrs.max {
                *input = new
            }
        }
    }

    pub fn dec(&mut self, pos: Position, step_size: StepSize) {
        let attrs = input_attrs(pos.column % INPUTS_PER_STEP);
        let sub = attrs.step_sizes[step_size as usize];
        let input = self.input(pos).get_or_insert(attrs.default);
        if let Some(new) = input.checked_sub(sub) {
            *input = new
        }
    }

    pub fn iter_notes(&self, tick: u64) -> impl Iterator<Item = NoteEvent> + '_ {
        let line = (tick % self.length as u64) as usize;
        self.tracks.iter().enumerate().flat_map(move |(i, track)| {
            track
                .steps
                .iter()
                .enumerate()
                .filter(move |(l, step)| *l == line && step.pitch.is_some())
                .map(move |(_, &step)| NoteEvent {
                    pitch: step.pitch.unwrap(),
                    track: i as u8,
                    sound: step.sound.unwrap_or(i as u8),
                })
        })
    }

    fn input(&mut self, pos: Position) -> &mut Option<u8> {
        let track = pos.column / INPUTS_PER_STEP;
        let track = &mut self.tracks[track];
        let step = &mut track.steps[pos.line];
        match pos.column % INPUTS_PER_STEP {
            0 => &mut step.pitch,
            1 => &mut step.sound,
            _ => unreachable!(),
        }
    }
}

struct InputAttrs {
    max: u8,
    default: u8,
    step_sizes: [u8; 2],
}

fn input_attrs(offset: usize) -> InputAttrs {
    match offset {
        0 => InputAttrs {
            max: MAX_PITCH,
            default: ROOT_PITCH,
            step_sizes: [1, 12],
        },
        _ => InputAttrs {
            max: 99,
            default: 0,
            step_sizes: [1, 1],
        },
    }
}

#[derive(Clone, Debug)]
pub struct Track {
    pub steps: Vec<Step>,
}

#[derive(Copy, Clone, Debug, Default)]
pub struct Step {
    pub pitch: Option<u8>,
    pub sound: Option<u8>,
}

pub struct NoteEvent {
    pub pitch: u8,
    pub sound: u8,
    pub track: u8,
}

#[derive(Copy, Clone)]
pub enum StepSize {
    Default = 0,
    Large,
}
