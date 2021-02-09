use std::sync::{atomic::Ordering, Arc};

use anyhow::{anyhow, Result};
use atomic_float::AtomicF32;

#[derive(Copy, Clone)]
pub enum Unit {
    Decibel,
    Seconds,
    Samples,
}

pub struct Param {
    min: f32,
    max: f32,
    pub val: Arc<AtomicF32>,
    step: f32,
    unit: Option<Unit>,
}

impl Param {
    pub fn new(min: f32, val: Arc<AtomicF32>, max: f32, step: f32) -> Self {
        Self {
            min,
            val,
            max,
            step,
            unit: None,
        }
    }

    pub fn with_unit(mut self, unit: Unit) -> Self {
        self.unit = Some(unit);
        self
    }

    pub fn incr(&mut self) {
        let mut val = self.val.load(Ordering::Relaxed);
        val = f32::min(val + self.step, self.max);
        self.val.store(val, Ordering::Relaxed);
    }

    pub fn decr(&mut self) {
        let mut val = self.val.load(Ordering::Relaxed);
        val = f32::max(val - self.step, self.min);
        self.val.store(val, Ordering::Relaxed);
    }

    pub fn set(&mut self, value: f32) -> Result<()> {
        if value > self.max || value < self.min {
            return Err(anyhow!(
                "value must be between {} and {}",
                self.min,
                self.max
            ));
        }
        self.val.store(value, Ordering::Relaxed);
        Ok(())
    }
}

impl std::fmt::Display for Param {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let val = self.val.load(Ordering::Relaxed);
        if let Some(unit) = self.unit {
            match unit {
                Unit::Decibel => write!(f, "{:.2} dB", val),
                Unit::Seconds => {
                    if val < 1.0 {
                        write!(f, "{:.0} ms", val * 1000.0)
                    } else {
                        write!(f, "{:.2} s", val)
                    }
                }
                Unit::Samples => write!(f, "{:.0}", val),
            }
        } else {
            write!(f, "{:.2}", val)
        }
    }
}
