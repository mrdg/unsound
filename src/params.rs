use std::sync::Arc;

use crate::pattern::StepSize;

pub struct ParamInfo {
    name: String,
    min: f64,
    max: f64,
    default: f64,
    steps: Option<[f64; 2]>,
    format_value: Option<FormatValue>,
}

impl ParamInfo {
    const DEFAULT_STEPS: [f64; 2] = [0.01, 0.1];

    pub fn new<T: Into<f64>>(name: &str, default: T, min: T, max: T) -> Self {
        Self {
            name: String::from(name),
            default: default.into(),
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

#[derive(Clone)]
pub struct Params {
    values: Vec<f64>,
    info: Arc<Vec<ParamInfo>>,
}

impl Params {
    pub fn value<I: Into<usize>>(&self, idx: I) -> f64 {
        self.values[idx.into()]
    }

    pub fn incr(&mut self, idx: usize, step_size: StepSize) {
        let info = &self.info[idx];
        let step = info.step(step_size);
        let v = &mut self.values[idx];
        *v = f64::min(info.max, *v + step);
    }

    pub fn decr(&mut self, idx: usize, step_size: StepSize) {
        let info = &self.info[idx];
        let step = info.step(step_size);
        let v = &mut self.values[idx];
        *v = f64::max(info.min, *v - step);
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = LabeledParam> {
        self.info.iter().enumerate().map(move |(i, info)| {
            let f = info.format_value.unwrap_or(format_default);
            LabeledParam {
                label: &info.name,
                value: f(self.values[i]),
            }
        })
    }
}

pub struct ParamsBuilder {
    values: Vec<f64>,
    info: Vec<ParamInfo>,
}

impl ParamsBuilder {
    pub fn new() -> Self {
        Self {
            values: Vec::new(),
            info: Vec::new(),
        }
    }

    pub fn insert<P: Into<usize>>(&mut self, param: P, info: ParamInfo) {
        let idx = param.into();
        self.values.insert(idx, info.default);
        self.info.insert(idx, info);
    }

    pub fn build(self) -> Params {
        Params {
            values: self.values,
            info: Arc::new(self.info),
        }
    }
}

pub struct LabeledParam<'a> {
    pub label: &'a String,
    pub value: String,
}
