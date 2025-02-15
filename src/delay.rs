use std::sync::Arc;

use crate::audio::Stereo;
use crate::engine::{Plugin, PluginEvent, ProcessContext, ProcessStatus};
use crate::params::{self, Params};
use param_derive::Params;

pub struct Delay {
    buffer: Vec<Stereo>,
    write_pos: usize,
    delay_samples: usize,
}

#[derive(Params)]
struct DelayParams {}

impl Delay {
    pub fn new(delay_samples: usize) -> Self {
        Delay {
            buffer: vec![Stereo::ZERO; delay_samples],
            write_pos: 0,
            delay_samples,
        }
    }
}

impl Plugin for Delay {
    fn send_event(&mut self, _event: PluginEvent) {}

    fn params(&self) -> Arc<dyn Params> {
        Arc::new(DelayParams {})
    }

    fn process(&mut self, ctx: &mut ProcessContext) -> ProcessStatus {
        const FEEDBACK: f32 = 0.5;
        const DRY_MIX: f32 = 0.8;
        const WET_MIX: f32 = 0.8;

        for mut frame in ctx.buffers() {
            let read_pos = {
                let mut pos = self.write_pos as isize - self.delay_samples as isize;
                if pos < 0 {
                    pos += self.delay_samples as isize;
                }
                pos as usize
            };

            let delayed_sample = self.buffer[read_pos];
            let output = *frame.input * DRY_MIX + delayed_sample * WET_MIX;
            frame.write(output);

            self.buffer[self.write_pos] = *frame.input + delayed_sample * FEEDBACK;
            self.write_pos = (self.write_pos + 1) % self.delay_samples;
        }

        ProcessStatus::Continue
    }
}
