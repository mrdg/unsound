use crate::app::HostParam;
use crate::sampler::SamplerParam;
use anyhow::{anyhow, Result};

#[derive(Copy, Clone, PartialEq)]
pub enum ParamKey {
    Host(HostParam),
    Sampler(SamplerParam),
}

impl std::fmt::Display for ParamKey {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Sampler(param) => param.fmt(f),
            _ => Ok(()),
        }
    }
}

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

    pub fn inc(&mut self) {
        self.val = f32::min(self.val + self.step, self.max);
    }

    pub fn dec(&mut self) {
        self.val = f32::max(self.val - self.step, self.min);
    }

    pub fn set(&mut self, value: f32) -> Result<()> {
        if value > self.max || value < self.min {
            return Err(anyhow!(
                "value must be between {} and {}",
                self.min,
                self.max
            ));
        }
        self.val = value;
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
