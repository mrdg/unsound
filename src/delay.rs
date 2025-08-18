use std::sync::Arc;

use crate::audio::Stereo;
use crate::engine::{Plugin, PluginEvent, ProcessContext, ProcessStatus};
use crate::params::{self, Param, ParamInfo, Params};
use param_derive::Params;

pub struct Delay {
    buffer: Vec<Stereo>,
    write_pos: usize,
    delay_samples: usize,
    params: Arc<DelayParams>,
}

#[derive(Params)]
pub struct DelayParams {
    dry_mix: Param,
    wet_mix: Param,
}

impl Default for DelayParams {
    fn default() -> Self {
        Self {
            dry_mix: Param::new(0.8, ParamInfo::new("Dry Mix", 0, 1)),
            wet_mix: Param::new(0.8, ParamInfo::new("Wet Mix", 0, 1)),
        }
    }
}

impl Delay {
    pub fn new(delay_samples: usize) -> Self {
        Delay {
            buffer: vec![Stereo::ZERO; delay_samples],
            write_pos: 0,
            delay_samples,
            params: Arc::new(DelayParams::default()),
        }
    }
}

impl Plugin for Delay {
    fn send_event(&mut self, _event: PluginEvent) {}

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn process(&mut self, ctx: &mut ProcessContext) -> ProcessStatus {
        const FEEDBACK: f32 = 0.5;

        for mut frame in ctx.buffers() {
            let read_pos = {
                let mut pos = self.write_pos as isize - self.delay_samples as isize;
                if pos < 0 {
                    pos += self.delay_samples as isize;
                }
                pos as usize
            };

            let delayed_sample = self.buffer[read_pos];

            let dry = self.params.dry_mix.value();
            let wet = self.params.wet_mix.value();
            let output = *frame.input * dry as f32 + delayed_sample * wet as f32;
            frame.write(output);

            self.buffer[self.write_pos] = *frame.input + delayed_sample * FEEDBACK;
            self.write_pos = (self.write_pos + 1) % self.delay_samples;
        }

        ProcessStatus::Continue
    }
}
