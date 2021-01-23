use std::error::Error;

#[derive(Copy, Clone)]
pub enum Unit {
    Decibel,
    Seconds,
    Samples,
}

#[derive(Copy, Clone)]
pub struct Param {
    min: f32,
    max: f32,
    pub val: f32,
    step: f32,
    unit: Option<Unit>,
}

impl Param {
    pub fn new(min: f32, val: f32, max: f32, step: f32) -> Self {
        Self {
            min,
            val,
            max,
            step,
            unit: None,
        }
    }

    pub fn with_unit(&mut self, unit: Unit) -> Self {
        self.unit = Some(unit);
        *self
    }

    pub fn up(&mut self) {
        self.val = f32::min(self.val + self.step, self.max);
    }

    pub fn down(&mut self) {
        self.val = f32::max(self.val - self.step, self.min);
    }

    pub fn set_from_string(&mut self, s: String) -> Result<(), Box<dyn Error>> {
        let new_val = s.parse()?;
        if new_val > self.max || new_val < self.min {
            return Err(format!("value must be between {} and {}", self.min, self.max).into());
        }
        self.val = new_val;
        Ok(())
    }
}

impl std::fmt::Display for Param {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        if let Some(unit) = self.unit {
            match unit {
                Unit::Decibel => write!(f, "{:.2} dB", self.val),
                Unit::Seconds => {
                    if self.val < 1.0 {
                        write!(f, "{:.0} ms", self.val * 1000.0)
                    } else {
                        write!(f, "{:.2} s", self.val)
                    }
                }
                Unit::Samples => write!(f, "{:.0}", self.val),
            }
        } else {
            write!(f, "{:.2}", self.val)
        }
    }
}

#[derive(Copy, Clone)]
pub enum ParamKey {
    Amp,
    AmpEnvAttack,
    AmpEnvDecay,
    AmpEnvSustain,
    AmpEnvRelease,
    SampleOffset,
}

impl std::fmt::Display for ParamKey {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let s = match self {
            Self::Amp => "Amp",
            Self::SampleOffset => "Sample Offset",
            Self::AmpEnvAttack => "Amp Env Attack",
            Self::AmpEnvDecay => "Amp Env Decay",
            Self::AmpEnvSustain => "Amp Env Sustain",
            Self::AmpEnvRelease => "Amp Env Release",
        };
        write!(f, "{}", s)
    }
}
