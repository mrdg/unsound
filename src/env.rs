use crate::{sampler::Adsr, SAMPLE_RATE};

#[derive(Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

// Amount by which we overshoot the target amplitude in the envelope
const EPS: f64 = 0.001;

// Envelope based on https://mu.krj.st/adsr/
#[derive(Debug)]
pub struct Envelope {
    pub state: State,

    prev_gate: f64,
    out: f64,
    pole: f64,
    target: f64,
    sustain_val: f64,

    pub attack: f64,
    pub decay: f64,
    pub sustain: f64,
    pub release: f64,
}

impl Envelope {
    pub fn new(attack: f64, decay: f64, sustain: f64, release: f64) -> Envelope {
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

    pub fn update(&mut self, adsr: &Adsr) {
        self.attack = adsr.attack;
        self.decay = adsr.decay;
        self.sustain = adsr.sustain;
        self.release = adsr.release;
    }

    pub fn value(&mut self, gate: f64) -> f64 {
        let sustain = self.sustain_value();

        if gate > self.prev_gate {
            self.state = State::Attack;
            self.target = 1.0 + EPS;
            self.pole = ratio_to_pole(self.attack, EPS / self.target);
        } else if gate < self.prev_gate {
            self.state = State::Release;
            self.target = -EPS;
            self.pole = ratio_to_pole(self.release, EPS / (sustain + EPS));
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

    fn sustain_value(&mut self) -> f64 {
        self.sustain_val = 0.001 * self.sustain + 0.999 * self.sustain_val;
        self.sustain_val
    }
}

fn ratio_to_pole(msec: f64, ratio: f64) -> f64 {
    f64::powf(ratio, 1.0 / ((msec / 1000.0) * SAMPLE_RATE as f64))
}
