use crate::pattern::StepSize;

use atomic_float::AtomicF64;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub trait Params {
    fn get_param(&self, index: usize) -> &Param;
    fn len(&self) -> usize;
}

pub struct Param {
    value: AtomicF64,
    info: ParamInfo,
}

impl Param {
    pub fn new(value: f64, info: ParamInfo) -> Self {
        Self {
            value: AtomicF64::new(value),
            info,
        }
    }
    pub fn incr(&self, step_size: StepSize) {
        let step = self.info.step(step_size);
        let new = f64::min(self.info.max, self.value() + step);
        self.value.store(new, Ordering::Relaxed);
    }

    pub fn decr(&self, step_size: StepSize) {
        let step = self.info.step(step_size);
        let new = f64::max(self.info.min, self.value() - step);
        self.value.store(new, Ordering::Relaxed);
    }

    pub fn value_as_string(&self) -> String {
        let fmt = self.info.format_value.unwrap_or(format_default);
        fmt(self.value())
    }

    pub fn value(&self) -> f64 {
        self.value.load(Ordering::Relaxed)
    }

    pub fn label(&self) -> &str {
        self.info.name.as_str()
    }
}

pub struct ParamInfo {
    name: String,
    min: f64,
    max: f64,
    steps: Option<[f64; 2]>,
    format_value: Option<FormatValue>,
}

impl ParamInfo {
    const DEFAULT_STEPS: [f64; 2] = [0.01, 0.1];

    pub fn new<T: Into<f64>>(name: &str, min: T, max: T) -> Self {
        Self {
            name: String::from(name),
            min: min.into(),
            max: max.into(),
            steps: None,
            format_value: None,
        }
    }

    pub fn with_steps<T: Into<f64>>(mut self, steps: [T; 2]) -> Self {
        self.steps = Some(steps.map(|s| s.into()));
        self
    }

    pub fn with_formatter(mut self, format_value: FormatValue) -> Self {
        self.format_value = Some(format_value);
        self
    }

    fn step(&self, step_size: StepSize) -> f64 {
        self.steps.unwrap_or(Self::DEFAULT_STEPS)[step_size as usize]
    }
}

type FormatValue = fn(f64) -> String;

fn format_default(v: f64) -> String {
    format!("{:.2}", v)
}

pub fn format_millis(v: f64) -> String {
    format!("{}ms", v)
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
