#[derive(Debug, PartialEq)]
pub enum State {
    Init,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Debug)]
pub struct Envelope {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,

    attack_rate: f32,
    pub decay_rate: f32,
    release_rate: f32,

    samples_after_release: i32,

    val: f32,
    pub state: State,
}

impl Envelope {
    pub fn new() -> Envelope {
        Envelope {
            attack: 0.01,
            decay: 0.1,
            sustain: 0.8,
            release: 0.01,
            attack_rate: 0.,
            decay_rate: 0.,
            release_rate: 0.,
            val: 0.,
            state: State::Init,
            samples_after_release: 0,
        }
    }

    pub fn value(&mut self) -> f32 {
        match self.state {
            State::Init => {
                return 0.0;
            }
            State::Attack => {
                self.val += self.attack_rate;
                if self.val >= 1.0 {
                    self.val = 1.0;
                    self.state = if self.decay_rate > 0.0 {
                        State::Decay
                    } else {
                        State::Sustain
                    }
                }
            }
            State::Decay => {
                self.val -= self.decay_rate;
                if self.val <= self.sustain {
                    self.val = self.sustain;
                    self.state = State::Sustain
                }
            }
            State::Sustain => {
                if self.sustain == 0.0 {
                    self.state = State::Init;
                } else {
                    self.val = self.sustain;
                }
            }
            State::Release => {
                self.samples_after_release -= 1;
                if self.samples_after_release <= 0 {
                    self.val = 0.0;
                } else {
                    self.val -= self.release_rate;
                }
                if self.val <= 0.0 {
                    self.val = 0.0;
                    self.state = State::Init;
                }
            }
        }
        return self.val;
    }

    pub fn start_attack(&mut self) {
        let sample_rate = super::SAMPLE_RATE as f32;
        self.val = 0.0;
        self.state = State::Attack;
        self.attack_rate = 1.0 / (self.attack * sample_rate);
        self.decay_rate = if self.sustain > 0.0 {
            1.0 - self.sustain / (self.decay * sample_rate)
        } else {
            1.0 / (self.decay * sample_rate)
        }
    }

    pub fn start_release(&mut self) {
        self.state = State::Release;
        self.samples_after_release = (self.release * super::SAMPLE_RATE as f32) as i32;
        self.release_rate = self.val / self.samples_after_release as f32;
    }
}
