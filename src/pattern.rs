use std::convert::{TryFrom, TryInto};
use std::fmt::Display;
use std::ops::{Add, Sub};

use num_traits::identities::Zero;

use crate::app::ViewContext;
use crate::sampler::ROOT_PITCH;

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

    pub fn iter_notes(&self, tick: u64) -> impl Iterator<Item = NoteEvent> + '_ {
        let line = (tick % self.len() as u64) as usize;
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
                    fx1: step.effect1,
                    fx2: step.effect2,
                })
        })
    }
}

#[derive(Clone, Debug)]
pub struct Track {
    pub steps: Vec<Step>,
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
    pub fn input(&mut self, pos: Position) -> Box<dyn Input + '_> {
        match pos.column % INPUTS_PER_STEP {
            0 => Box::new(PitchInput::new(&mut self.pitch)),
            1 => Box::new(NumInput::new(&mut self.sound, 0, 0, 99, [1, 10])),
            2 => Box::new(EffectInput::new(&mut self.effect1)),
            3 => Self::effect_value_input(&mut self.effect1),
            4 => Box::new(EffectInput::new(&mut self.effect2)),
            5 => Self::effect_value_input(&mut self.effect2),
            _ => unreachable!(),
        }
    }

    fn effect_value_input<'a>(effect: &'a mut Option<Effect>) -> Box<dyn Input + 'a> {
        if let Some(effect) = effect {
            match effect {
                Effect::Chord(c) => Box::new(NumInput::new(c, 0, 0, 999, [1, 10])),
                Effect::Velocity(v) => {
                    Box::new(NumInput::new(v, DEFAULT_VELOCITY, 0, 127, [1, 10]))
                }
            }
        } else {
            Box::new(NullInput {})
        }
    }
}

pub trait Input {
    fn keypress(&mut self, ctx: ViewContext, key: char);
    fn clear(&mut self);
    fn next(&mut self, step_size: StepSize);
    fn prev(&mut self, step_size: StepSize);
}

struct NumInput<'a, T> {
    value: &'a mut Option<T>,
    default: T,
    min: T,
    max: T,
    steps: [T; 2],
}

impl<'a, T> NumInput<'a, T> {
    fn new(value: &'a mut Option<T>, default: T, min: T, max: T, steps: [T; 2]) -> Self {
        Self {
            value,
            default,
            min,
            max,
            steps,
        }
    }
}

impl<T> Input for NumInput<'_, T>
where
    T: Add<Output = T> + Sub<Output = T> + Zero,
    T: TryFrom<i64> + Into<i64>,
    T: PartialOrd + Copy,
{
    fn clear(&mut self) {
        *self.value = None;
    }

    fn keypress(&mut self, _ctx: ViewContext, key: char) {
        let new = match (&self.value, key.to_digit(10)) {
            (Some(val), Some(d)) => {
                let val: i64 = (*val).into();
                let d = d as i64;
                let d = if val < 0 { -d } else { d };
                val * 10 + d as i64
            }
            (None, Some(d)) => d as i64,
            (Some(val), None) if key == '-' && self.min < T::zero() => {
                let val: i64 = (*val).into();
                -val
            }
            _ => return,
        };
        // restrict value length to 3 chars to fit in the editor
        let new = if new > 0 { new % 1000 } else { new % 100 };
        if let Ok(n) = new.try_into() {
            if n > self.max || n < self.min {
                // TODO: show error?
                return;
            }
            *self.value = Some(n);
        }
    }

    fn next(&mut self, step_size: StepSize) {
        let curr = self.value.unwrap_or(self.default);
        let mut new = curr + self.steps[step_size as usize];
        if new > self.max {
            new = self.max;
        }
        *self.value = Some(new);
    }

    fn prev(&mut self, step_size: StepSize) {
        let curr = self.value.unwrap_or(self.default);
        let decr = self.steps[step_size as usize];
        if self.min + decr > curr {
            *self.value = Some(self.min);
        } else {
            *self.value = Some(curr - decr);
        }
    }
}

struct PitchInput<'a> {
    input: NumInput<'a, u8>,
}

impl<'a> PitchInput<'a> {
    fn new(value: &'a mut Option<u8>) -> Self {
        let input = NumInput::new(value, ROOT_PITCH, 0, 109, [1, 12]);
        Self { input }
    }
}

impl Input for PitchInput<'_> {
    fn keypress(&mut self, ctx: ViewContext, key: char) {
        if let Some(pitch) = key_to_pitch(key) {
            let new = ctx.octave() * 12 + pitch as u16;
            *self.input.value = Some(new as u8);
        } else if key == 'a' {
            *self.input.value = Some(NOTE_OFF)
        }
    }

    fn clear(&mut self) {
        self.input.clear();
    }

    fn next(&mut self, step_size: StepSize) {
        self.input.next(step_size);
    }

    fn prev(&mut self, step_size: StepSize) {
        self.input.prev(step_size);
    }
}

fn key_to_pitch(key: char) -> Option<u8> {
    let pitch = match key {
        'z' => 0,
        's' => 1,
        'x' => 2,
        'd' => 3,
        'c' => 4,
        'v' => 5,
        'g' => 6,
        'b' => 7,
        'h' => 8,
        'n' => 9,
        'j' => 10,
        'm' => 11,
        _ => return None,
    };
    Some(pitch)
}

struct NullInput {}

impl Input for NullInput {
    fn keypress(&mut self, _ctx: ViewContext, _key: char) {}
    fn clear(&mut self) {}
    fn next(&mut self, _step_size: StepSize) {}
    fn prev(&mut self, _step_size: StepSize) {}
}

#[derive(Copy, Clone, Debug)]
pub enum Effect {
    Chord(Option<i16>),
    Velocity(Option<u8>),
}

impl Effect {
    pub fn desc(&self) -> EffectDesc {
        match self {
            Effect::Chord(c) => EffectDesc::new('C', *c),
            Effect::Velocity(v) => EffectDesc::new('V', *v),
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

struct EffectInput<'a> {
    value: &'a mut Option<Effect>,
}

impl<'a> EffectInput<'a> {
    fn new(value: &'a mut Option<Effect>) -> Self {
        Self { value }
    }
}

impl<'a> Input for EffectInput<'a> {
    fn keypress(&mut self, _ctx: ViewContext, key: char) {
        let new = match key {
            'V' => Effect::Velocity(None),
            'C' => Effect::Chord(None),
            _ => return,
        };
        *self.value = Some(new);
    }

    // TODO: implement next/prev
    fn next(&mut self, _step_size: StepSize) {}
    fn prev(&mut self, _step_size: StepSize) {}

    fn clear(&mut self) {
        *self.value = None;
    }
}
