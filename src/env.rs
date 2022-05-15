use std::time::Duration;

use crate::SAMPLE_RATE;

#[derive(Debug, PartialEq)]
pub enum State {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

// Amount by which we overshoot the target amplitude in the envelope
const EPS: f32 = 0.001;

// Envelope based on https://mu.krj.st/adsr/
#[derive(Debug)]
pub struct Envelope {
    pub state: State,

    prev_gate: f32,
    out: f32,
    pole: f32,
    target: f32,
    sustain_val: f32,

    pub attack: Duration,
    pub decay: Duration,
    pub sustain: f32,
    pub release: Duration,
}

impl Envelope {
    pub fn new(attack: Duration, decay: Duration, sustain: f32, release: Duration) -> Envelope {
        Envelope {
            state: State::Idle,
            attack,
            decay,
            sustain,
            release,
            out: 0.0,
            prev_gate: 0.0,
            pole: 0.0,
            sustain_val: 0.0,
            target: 0.0,
        }
    }

    pub fn value(&mut self, gate: f32) -> f32 {
        let sustain = self.sustain_value();

        if gate > self.prev_gate {
            self.state = State::Attack;
            self.target = 1.0 + EPS;
            self.pole = ratio_to_pole(self.attack, EPS / self.target);
        } else if gate < self.prev_gate {
            self.state = State::Release;
            self.target = -EPS;
            self.pole = ratio_to_pole(self.release, EPS / sustain + EPS);
        }

        self.prev_gate = gate;
        self.out = (1.0 - self.pole) * self.target + self.pole * self.out;

        use State::*;
        match self.state {
            Idle => return 0.0,
            Attack => {
                if self.out >= 1.0 {
                    self.out = 1.0;
                    self.state = Decay;
                    self.target = sustain - EPS;
                    self.pole = ratio_to_pole(self.decay, EPS / (1.0 - sustain + EPS));
                }
            }
            Decay => {
                if self.out <= sustain {
                    self.out = sustain;
                    self.state = Sustain;
                }
            }
            Sustain => {
                self.out = sustain;
            }
            Release => {
                if self.out <= 0.0 {
                    self.out = 0.0;
                    self.state = Idle;
                }
            }
        };

        self.out
    }

    fn sustain_value(&mut self) -> f32 {
        self.sustain_val = 0.001 * self.sustain + 0.999 * self.sustain_val;
        self.sustain_val
    }
}

fn ratio_to_pole(t: Duration, ratio: f32) -> f32 {
    f32::powf(ratio, 1.0 / (t.as_secs_f32() * SAMPLE_RATE as f32))
}
