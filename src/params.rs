use crate::pattern::StepSize;

use atomic_float::AtomicF64;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub trait Params {
    fn get_param(&self, index: usize) -> &Param;
    fn len(&self) -> usize;
}

pub struct Param {
    current: AtomicF64,
    target: AtomicF64,
    info: ParamInfo,
}

impl Param {
    pub fn new(value: f64, info: ParamInfo) -> Self {
        Self {
            target: AtomicF64::new(value),
            current: AtomicF64::new(value),
            info,
        }
    }

    pub fn incr(&self, step_size: StepSize) {
        let step = self.info.step(step_size);
        let new = f64::min(self.info.max, self.target() + step);
        self.set(new);
    }

    pub fn decr(&self, step_size: StepSize) {
        let step = self.info.step(step_size);
        let new = f64::max(self.info.min, self.target() - step);
        self.set(new);
    }

    fn set(&self, value: f64) {
        if value >= self.info.min && value <= self.info.max {
            self.target.store(value, Ordering::Relaxed);
        }
    }

    pub fn value(&self) -> f64 {
        let mut current = self.current.load(Ordering::Relaxed);
        let target = self.target.load(Ordering::Relaxed);
        current = self.info.smoothing.next(current, target);
        self.current.store(current, Ordering::Relaxed);
        (self.info.map_value)(current)
    }

    pub fn target(&self) -> f64 {
        self.target.load(Ordering::Relaxed)
    }

    pub fn toggle(&self) {
        assert_eq!(self.info.min, 0.0);
        assert_eq!(self.info.max, 1.0);
        let val = self.target();
        self.set((val + -1.0).abs());
    }

    pub fn label(&self) -> &str {
        self.info.name.as_str()
    }

    pub fn as_string(&self) -> String {
        (self.info.format_value)(self.target())
    }

    pub fn as_bool(&self) -> bool {
        self.target() == self.info.true_value
    }
}

pub struct ParamInfo {
    name: String,
    min: f64,
    max: f64,
    steps: [f64; 2],
    format_value: Box<FormatValue>,
    map_value: Box<MapValue>,
    smoothing: Box<dyn Smoothing + Send + Sync>,
    true_value: f64,
}

impl ParamInfo {
    const DEFAULT_STEPS: [f64; 2] = [0.01, 0.1];

    pub fn new<T: Into<f64>>(name: &str, min: T, max: T) -> Self {
        Self {
            name: String::from(name),
            min: min.into(),
            max: max.into(),
            steps: Self::DEFAULT_STEPS,
            format_value: Box::new(format_default),
            smoothing: Box::new(NoSmoothing),
            map_value: Box::new(|v| v),
            true_value: 1.0,
        }
    }

    pub fn bool(name: &str, true_value: f64) -> Self {
        let mut info = Self::new(name, 0.0, 1.0).with_steps([1.0, 1.0]);
        info.true_value = true_value;
        info
    }

    pub fn with_steps<T: Into<f64>>(mut self, steps: [T; 2]) -> Self {
        self.steps = steps.map(|s| s.into());
        self
    }

    pub fn with_formatter<F>(mut self, format: F) -> Self
    where
        F: Fn(f64) -> String,
        F: Send + Sync + 'static,
    {
        self.format_value = Box::new(format);
        self
    }

    pub fn with_map<F>(mut self, map: F) -> Self
    where
        F: Fn(f64) -> f64,
        F: Send + Sync + 'static,
    {
        self.map_value = Box::new(map);
        self
    }

    pub fn with_smoothing<S>(mut self, smoothing: S) -> Self
    where
        S: Smoothing + Send + Sync + 'static,
    {
        self.smoothing = Box::new(smoothing);
        self
    }

    fn step(&self, step_size: StepSize) -> f64 {
        self.steps[step_size as usize]
    }
}

type FormatValue = dyn Fn(f64) -> String + Send + Sync;
type MapValue = dyn Fn(f64) -> f64 + Send + Sync;

pub fn db_to_amp(db: f64) -> f64 {
    f64::powf(10.0, db / 20.0)
}

fn format_default(v: f64) -> String {
    format!("{:.2}", v)
}

pub fn format_millis(v: f64) -> String {
    format!("{}ms", v)
}

pub trait Smoothing {
    fn next(&self, current: f64, target: f64) -> f64;
}

pub struct ExpSmoothing {
    rate: f64,
}

impl ExpSmoothing {
    pub fn new(ms: f64, sample_rate: f64) -> Self {
        let num_samples = (sample_rate * ms / 1000.0).round();
        let rate = 0.0001f64.powf(1.0 / num_samples);
        Self { rate }
    }
}

impl Default for ExpSmoothing {
    fn default() -> Self {
        Self::new(5.0, crate::SAMPLE_RATE)
    }
}

impl Smoothing for ExpSmoothing {
    fn next(&self, current: f64, target: f64) -> f64 {
        let mut current = self.rate * current + (1.0 - self.rate) * target;
        if (target - current).abs() < 0.0001 {
            current = target;
        }
        current
    }
}

struct NoSmoothing;

impl Smoothing for NoSmoothing {
    fn next(&self, _current: f64, target: f64) -> f64 {
        target
    }
}

pub struct ParamIter<'a> {
    current: usize,
    params: &'a Arc<dyn Params>,
}

impl<'a> Iterator for ParamIter<'a> {
    type Item = &'a Param;
    fn next(&mut self) -> Option<Self::Item> {
        if self.current >= self.params.len() {
            None
        } else {
            let idx = self.current;
            self.current += 1;
            Some(self.params.get_param(idx))
        }
    }
}

pub trait ParamIterExt {
    fn iter(&self) -> ParamIter;
}

impl ParamIterExt for Arc<dyn Params> {
    fn iter(&self) -> ParamIter {
        ParamIter {
            current: 0,
            params: self,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mapping() {
        let param = Param::new(
            1.0,
            ParamInfo::new("Test", 0.0, 100.0)
                .with_steps([1.0, 5.0])
                .with_map(|v| v * 2.0),
        );
        assert_eq!(2.0, param.value());
        param.incr(StepSize::Default);
        assert_eq!(4.0, param.value());
    }

    #[test]
    fn test_smoothing() {
        let time = 1.0;
        let sample_rate = 44100.0;
        let initial = 1.0;
        let target = 2.0;

        let param = Param::new(
            initial,
            ParamInfo::new("Test", 0.0, 100.0)
                .with_steps([1.0, 5.0])
                .with_smoothing(ExpSmoothing::new(time, sample_rate)),
        );
        param.incr(StepSize::Default);
        assert_eq!(target, param.target());

        let mut previous = f64::MIN;
        for _ in 0..(sample_rate * time / 1000.0).round() as usize {
            let current = param.value();
            assert!(current > previous);
            assert!(current <= target);
            previous = current;
        }
        assert_eq!(target, previous);
    }
}
