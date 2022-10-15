use num_traits::Zero;

use crate::engine::TICKS_PER_LINE;
use crate::pattern::{
    Effect, Position, Step, StepSize, DEFAULT_VELOCITY, INPUTS_PER_STEP, NOTE_OFF,
};
use crate::sampler::ROOT_PITCH;
use crate::view::context::ViewContext;

use std::convert::{TryFrom, TryInto};
use std::ops::{Add, Sub};

pub trait StepInput {
    fn input(&mut self, pos: Position) -> Box<dyn Input + '_>;
}

impl StepInput for Step {
    fn input(&mut self, pos: Position) -> Box<dyn Input + '_> {
        match pos.column % INPUTS_PER_STEP {
            0 => Box::new(PitchInput::new(&mut self.pitch)),
            1 => Box::new(NumInput::new(&mut self.sound, 0, 0, 99, [1, 10])),
            2 => Box::new(EffectInput::new(&mut self.effect1)),
            3 => effect_value_input(&mut self.effect1),
            4 => Box::new(EffectInput::new(&mut self.effect2)),
            5 => effect_value_input(&mut self.effect2),
            _ => unreachable!(),
        }
    }
}

fn effect_value_input<'a>(effect: &'a mut Option<Effect>) -> Box<dyn Input + 'a> {
    if let Some(effect) = effect {
        match effect {
            Effect::Chord(c) => Box::new(NumInput::new(c, 0, 0, 999, [1, 10])),
            Effect::Velocity(v) => Box::new(NumInput::new(v, DEFAULT_VELOCITY, 0, 127, [1, 10])),
            Effect::Offset(o) => {
                let max = TICKS_PER_LINE as u8 - 1;
                Box::new(NumInput::new(o, 0, 0, max, [1, 10]))
            }
        }
    } else {
        Box::new(NullInput {})
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
            'o' => Effect::Offset(None),
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
