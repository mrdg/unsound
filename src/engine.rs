use std::sync::Arc;

use ringbuf::Consumer;
use triple_buffer::{Input, Output};

use crate::app::{AppState, AudioContext, EngineState, SharedState};
use crate::pattern::MAX_TRACKS;
use crate::sampler::{Sampler, Sound, ROOT_PITCH};
use crate::SAMPLE_RATE;

pub enum EngineCommand {
    PreviewSound(Arc<Sound>),
}

pub trait Device {
    fn render(&mut self, buffer: &mut [(f32, f32)]);
}

pub struct Engine {
    state: EngineState,
    state_buf: Input<EngineState>,
    app_state_buf: Output<AppState>,
    channels: Vec<Sampler>,
    preview: Sampler,
    consumer: Consumer<EngineCommand>,
    samples_to_tick: usize,
}

impl Engine {
    pub fn new(
        state: EngineState,
        state_buf: Input<EngineState>,
        app_state_buf: Output<AppState>,
        consumer: Consumer<EngineCommand>,
    ) -> Engine {
        let mut channels = Vec::with_capacity(MAX_TRACKS);
        for _ in 0..MAX_TRACKS {
            let sampler = Sampler::new();
            channels.push(sampler);
        }
        Self {
            state,
            state_buf,
            app_state_buf,
            channels,
            preview: Sampler::new(),
            consumer,
            samples_to_tick: 0,
        }
    }

    pub fn render(&mut self, buffer: &mut [(f32, f32)]) {
        self.run_commands();

        let mut buffer = buffer;
        while let Some(block_size) = self.next_block(buffer.len()) {
            for chan in &mut self.channels {
                chan.render(&mut buffer[..block_size]);
            }
            self.preview.render(&mut buffer[..block_size]);
            buffer = &mut buffer[block_size..];
        }

        let buf = self.state_buf.input_buffer();
        buf.clone_from(&self.state);
        self.state_buf.publish();
    }

    fn next_block(&mut self, frames: usize) -> Option<usize> {
        let ctx = AudioContext::new(self.app_state_buf.read(), &self.state);

        if self.samples_to_tick == 0 && ctx.is_playing() {
            let mut current_tick = ctx.current_line();
            let mut active_idx = ctx.active_pattern_index();

            let pattern = ctx.pattern(active_idx).unwrap_or_else(|| {
                // The active pattern can be deleted while we're playing it. Continue with the
                // next one if that happens, which should always be safe to unwrap.
                active_idx = ctx.next_pattern();
                ctx.pattern(active_idx).unwrap()
            });

            for note in pattern.iter_notes(current_tick as u64) {
                if let Some(Some(snd)) = ctx.sounds().get(note.sound as usize) {
                    let sampler = &mut self.channels[note.track as usize];
                    sampler.note_on(snd.to_owned(), note.track as usize, note.pitch, 90);
                }
            }
            let samples_to_tick = (SAMPLE_RATE * 60.) / (ctx.lines_per_beat() * ctx.bpm()) as f64;
            self.samples_to_tick = samples_to_tick.round() as usize;

            current_tick += 1;
            if current_tick >= pattern.length {
                current_tick = 0;
                active_idx = ctx.next_pattern();
            }
            self.state.current_tick = current_tick;
            self.state.current_pattern = active_idx;
        }

        let block_size = usize::min(frames, self.samples_to_tick);
        self.samples_to_tick -= block_size;
        if block_size > 0 {
            Some(block_size)
        } else {
            None
        }
    }

    fn run_commands(&mut self) {
        while let Some(cmd) = self.consumer.pop() {
            match cmd {
                EngineCommand::PreviewSound(snd) => {
                    self.preview.note_on(snd, 0, ROOT_PITCH, 80);
                }
            }
        }
    }
}
