use crate::pattern::MAX_TRACKS;
use crate::sampler::{Sampler, Sound, ROOT_PITCH};
use crate::state::{EngineControl, SharedState};
use crate::SAMPLE_RATE;
use basedrop::Shared;

pub enum EngineCommand {
    PreviewSound(Shared<Sound>),
}

pub trait Device {
    fn render(&mut self, buffer: &mut [(f32, f32)]);
}

#[derive(Debug)]
pub struct Block {
    pub start: usize,
    pub end: usize,
}

pub struct Engine {
    control: EngineControl,
    channels: Vec<Sampler>,
    preview: Sampler,
    samples_to_tick: usize,
}

impl Engine {
    pub fn new(control: EngineControl) -> Engine {
        let mut channels = Vec::with_capacity(MAX_TRACKS);
        for _ in 0..MAX_TRACKS {
            let sampler = Sampler::new();
            channels.push(sampler);
        }
        Self {
            control,
            channels,
            preview: Sampler::new(),
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
    }

    fn next_block(&mut self, frames: usize) -> Option<usize> {
        if self.samples_to_tick == 0 && self.control.is_playing() {
            let pattern = self.control.pattern();
            for note in pattern.iter_notes(self.control.current_tick()) {
                if let Some(snd) = self.control.sound(note.sound as usize) {
                    let sampler = &mut self.channels[note.track as usize];
                    sampler.note_on(snd.to_owned(), note.track as usize, note.pitch, 80);
                }
            }
            let samples_to_tick =
                (SAMPLE_RATE * 60.) / (self.control.lines_per_beat() * self.control.bpm()) as f64;
            self.samples_to_tick = samples_to_tick.round() as usize;
            self.control.tick();
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
        while let Some(cmd) = self.control.command() {
            match cmd {
                EngineCommand::PreviewSound(snd) => {
                    self.preview.note_on(snd, 0, ROOT_PITCH, 80);
                }
            }
        }
    }
}
