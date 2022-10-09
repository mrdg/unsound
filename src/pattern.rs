use crate::engine::TICKS_PER_LINE;
use std::fmt::Display;

pub const INPUTS_PER_STEP: usize = 6;
pub const MAX_PITCH: u8 = 109;
pub const NOTE_OFF: u8 = MAX_PITCH;
pub const MAX_PATTERNS: usize = 256;
pub const DEFAULT_VELOCITY: u8 = 100;

const DEFAULT_PATTERN_LEN: usize = 16;
const MAX_PATTERN_LEN: usize = 512;

#[derive(Clone, Copy, Debug, Default)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

impl Position {
    pub fn track(&self) -> usize {
        self.column / INPUTS_PER_STEP
    }

    pub fn is_pitch_input(&self) -> bool {
        self.column % INPUTS_PER_STEP == 0
    }
}

#[derive(Clone, Debug)]
pub struct Pattern {
    pub tracks: Vec<Track>,
}

impl Pattern {
    pub fn new(num_tracks: usize) -> Self {
        let mut tracks = Vec::with_capacity(num_tracks);
        for _ in 0..num_tracks {
            tracks.push(Track {
                steps: vec![Step::default(); DEFAULT_PATTERN_LEN],
            })
        }
        Self { tracks }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.len(), self.tracks.len() * INPUTS_PER_STEP)
    }

    pub fn len(&self) -> usize {
        self.tracks[0].steps.len()
    }

    pub fn ticks(&self) -> usize {
        self.len() * TICKS_PER_LINE
    }

    pub fn set_len(&mut self, new_len: usize) {
        if new_len > MAX_PATTERN_LEN {
            // TODO: return error
            return;
        }
        for track in &mut self.tracks {
            track.steps.resize(new_len, Step::default())
        }
    }

    pub fn steps(&self, track_idx: usize) -> &Vec<Step> {
        &self.tracks[track_idx].steps
    }

    pub fn step(&self, pos: Position) -> Step {
        self.tracks[pos.track()].steps[pos.line]
    }

    pub fn set_step(&mut self, pos: Position, step: Step) {
        self.tracks[pos.track()].steps[pos.line] = step;
    }

    // For each track in the pattern, return notes that should be played on the given tick. The
    // tick is relative to the start of the pattern.
    pub fn notes(&self, tick: usize) -> impl Iterator<Item = NoteEvent> + '_ {
        let line = tick / TICKS_PER_LINE;
        self.tracks.iter().enumerate().flat_map(move |(i, track)| {
            let step = &track.steps[line];
            let line_tick = line * TICKS_PER_LINE;

            let mut has_offset = false;
            let offset_match = step.offsets().any(|offset| {
                has_offset = true;
                line_tick + offset as usize == tick
            });

            if (!has_offset && line_tick == tick) || offset_match {
                step.pitch.map(|pitch| NoteEvent {
                    pitch,
                    track: i as u8,
                    sound: step.sound.unwrap_or(i as u8),
                    fx1: step.effect1,
                    fx2: step.effect2,
                })
            } else {
                None
            }
        })
    }
}

#[derive(Clone, Debug)]
pub struct Track {
    steps: Vec<Step>,
}

pub struct NoteEvent {
    pub pitch: u8,
    pub sound: u8,
    pub track: u8,
    pub fx1: Option<Effect>,
    pub fx2: Option<Effect>,
}

#[derive(Copy, Clone)]
pub enum StepSize {
    Default = 0,
    Large,
}

#[derive(Copy, Clone, Debug, Default)]
pub struct Step {
    pub pitch: Option<u8>,
    pub sound: Option<u8>,
    pub effect1: Option<Effect>,
    pub effect2: Option<Effect>,
}

impl Step {
    fn offsets(&self) -> impl Iterator<Item = u8> + '_ {
        self.effect1
            .iter()
            .chain(self.effect2.iter())
            .flat_map(|effect| {
                if let Effect::Offset(offset) = effect {
                    Some(*offset)
                } else {
                    None
                }
            })
            .flatten()
    }
}

#[derive(Copy, Clone, Debug)]
pub enum Effect {
    Chord(Option<i16>),
    Velocity(Option<u8>),
    Offset(Option<u8>),
}

impl Effect {
    pub fn desc(&self) -> EffectDesc {
        match self {
            Effect::Chord(c) => EffectDesc::new('C', *c),
            Effect::Velocity(v) => EffectDesc::new('V', *v),
            Effect::Offset(o) => EffectDesc::new('o', *o),
        }
    }
}

pub struct EffectDesc {
    pub effect_type: String,
    pub value: Option<String>,
}

impl EffectDesc {
    fn new<S: Into<String>, D: Display>(effect_type: S, value: Option<D>) -> Self {
        Self {
            effect_type: effect_type.into(),
            value: value.map(|v| format!("{:3}", v)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(track: usize, line: usize) -> Position {
        Position {
            line,
            column: INPUTS_PER_STEP * track,
        }
    }

    #[test]
    fn note_on_line() {
        let mut pattern = Pattern::new(2);
        let mut step = Step::default();
        for track in 0..2 {
            step.pitch = Some(60);
            pattern.set_step(position(track, 0), step);
        }

        let notes: Vec<NoteEvent> = pattern.notes(0).collect();
        let pitches: Vec<u8> = notes.iter().map(|n| n.pitch).collect();
        assert_eq!(vec![60, 60], pitches);
        let tracks: Vec<u8> = notes.iter().map(|n| n.track).collect();
        assert_eq!(vec![0, 1], tracks);
    }

    #[test]
    fn note_with_offset() {
        let mut pattern = Pattern::new(1);
        let mut step = Step::default();
        step.pitch = Some(60);
        step.effect1 = Some(Effect::Offset(Some(2)));
        pattern.set_step(position(0, 0), step);
        let notes: Vec<NoteEvent> = pattern.notes(2).collect();
        assert_eq!(1, notes.len());
    }

    #[test]
    fn note_with_offset_but_no_match() {
        let mut pattern = Pattern::new(1);
        let mut step = Step::default();
        step.pitch = Some(60);
        step.effect1 = Some(Effect::Offset(Some(2)));
        pattern.set_step(position(0, 0), step);
        let notes: Vec<NoteEvent> = pattern.notes(0).collect();
        assert_eq!(0, notes.len());
    }

    #[test]
    fn note_with_two_offsets() {
        let mut pattern = Pattern::new(1);
        let mut step = Step::default();
        step.pitch = Some(60);
        step.effect1 = Some(Effect::Offset(Some(2)));
        step.effect2 = Some(Effect::Offset(Some(3)));

        pattern.set_step(position(0, 0), step);
        let notes: Vec<NoteEvent> = pattern.notes(2).collect();
        assert_eq!(1, notes.len());

        let notes: Vec<NoteEvent> = pattern.notes(3).collect();
        assert_eq!(1, notes.len());
    }
}
