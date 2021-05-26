use crate::pattern::{Editor, Position, MAX_TRACKS};
use crate::SAMPLE_RATE;
use crate::{
    app::AppCommand,
    sampler::{Sampler, Sound, ROOT_PITCH},
};
use ringbuf::{Consumer, Producer};
use std::sync::{
    atomic::{AtomicBool, AtomicU16, Ordering},
    Arc,
};

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub enum EngineParam {
    Bpm,
    Octave,
    LinesPerBeat,
}

pub enum EngineCommand {
    InputNote(Position, u8),
    InputNumber(Position, i32),
    ChangeValue(Position, i32),
    DeleteValue(Position),
    LoadSound(usize, Arc<Sound>),
    PreviewSound(Arc<Sound>),
}

pub trait Device {
    fn render(&mut self, buffer: &mut [(f32, f32)]);
}

#[derive(Clone)]
pub struct EngineParams {
    pub bpm: Arc<AtomicU16>,
    pub lines_per_beat: Arc<AtomicU16>,
    pub octave: Arc<AtomicU16>,
    pub is_playing: Arc<AtomicBool>,
}

impl Default for EngineParams {
    fn default() -> Self {
        Self {
            bpm: Arc::new(AtomicU16::new(120)),
            octave: Arc::new(AtomicU16::new(4)),
            lines_per_beat: Arc::new(AtomicU16::new(4)),
            is_playing: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl EngineParams {
    pub fn set(&mut self, key: EngineParam, value: u16) {
        match key {
            EngineParam::Bpm => self.bpm.store(value, Ordering::Relaxed),
            EngineParam::Octave => self.octave.store(value, Ordering::Relaxed),
            EngineParam::LinesPerBeat => self.lines_per_beat.store(value, Ordering::Relaxed),
        }
    }

    pub fn get(&self, key: EngineParam) -> u16 {
        match key {
            EngineParam::Bpm => self.bpm.load(Ordering::Relaxed),
            EngineParam::Octave => self.octave.load(Ordering::Relaxed),
            EngineParam::LinesPerBeat => self.lines_per_beat.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug)]
pub struct Block {
    pub start: usize,
    pub end: usize,
}

pub struct Engine {
    cons: Consumer<EngineCommand>,
    prod: Producer<AppCommand>,

    editor: Editor,
    channels: Vec<Sampler>,
    sounds: Vec<Option<Arc<Sound>>>,

    preview: Sampler,

    params: EngineParams,

    samples_to_tick: usize,
    current_tick: u64,
}

impl Engine {
    pub fn new(
        params: EngineParams,
        cons: Consumer<EngineCommand>,
        prod: Producer<AppCommand>,
    ) -> Engine {
        let mut channels = Vec::with_capacity(MAX_TRACKS);
        for _ in 0..MAX_TRACKS {
            let sampler = Sampler::new();
            channels.push(sampler);
        }
        Self {
            cons,
            prod,
            editor: Editor::new(),
            channels,
            sounds: vec![None; MAX_TRACKS],
            preview: Sampler::new(),
            params,
            samples_to_tick: 0,
            current_tick: 0,
        }
    }

    pub fn render(&mut self, buffer: &mut [(f32, f32)]) {
        self.run_commands();
        let mut block = Block { start: 0, end: 0 };
        while self.next_block(&mut block, buffer.len()) {
            for chan in &mut self.channels {
                chan.render(&mut buffer[block.start..block.end]);
            }
            self.preview.render(&mut buffer[block.start..block.end]);
        }
    }

    pub fn run_commands(&mut self) {
        while let Some(update) = self.cons.pop() {
            match update {
                EngineCommand::LoadSound(index, sound) => {
                    self.sounds[index] = Some(sound);
                }
                EngineCommand::InputNote(pos, pitch) => {
                    self.editor.set_cursor(pos);
                    self.editor.set_pitch(pitch);
                }
                EngineCommand::InputNumber(pos, num) => {
                    self.editor.set_cursor(pos);
                    self.editor.set_number(num);
                }
                EngineCommand::ChangeValue(pos, delta) => {
                    self.editor.set_cursor(pos);
                    self.editor.change_value(delta);
                }
                EngineCommand::DeleteValue(pos) => {
                    self.editor.set_cursor(pos);
                    self.editor.delete_value();
                }
                EngineCommand::PreviewSound(snd) => {
                    self.preview.note_on(snd, 0, ROOT_PITCH, 80);
                }
            }
        }
    }

    pub fn next_block(&mut self, block: &mut Block, num_frames: usize) -> bool {
        if !self.params.is_playing.load(Ordering::Relaxed) {
            if block.end == num_frames {
                return false;
            }
            block.end = num_frames;
            return true;
        }

        self.samples_to_tick -= block.end - block.start;
        if block.end == num_frames {
            return false;
        }
        if block.end != 0 {
            block.start = block.end;
        }

        if self.samples_to_tick == 0 {
            for note in self.editor.iter_notes(self.current_tick) {
                if let Some(Some(snd)) = &self.sounds.get(note.sound as usize) {
                    let sampler = &mut self.channels[note.track as usize];
                    sampler.note_on(snd.clone(), note.track as usize, note.pitch, 80);
                }
            }
            let bpm = self.params.get(EngineParam::Bpm);
            let lines_per_beat = self.params.get(EngineParam::LinesPerBeat);
            let num_samples = (SAMPLE_RATE * 60.) / (lines_per_beat * bpm) as f64;
            self.samples_to_tick = num_samples.round() as usize;
            self.app_send(AppCommand::SetCurrentTick(self.current_tick as usize));
            self.current_tick += 1;
        }

        block.end = block.start + self.samples_to_tick;
        if block.end > num_frames {
            block.end = num_frames;
        }
        true
    }

    fn app_send(&mut self, cmd: AppCommand) {
        if self.prod.push(cmd).is_err() {
            eprintln!("unable to update client state");
        }
    }
}
