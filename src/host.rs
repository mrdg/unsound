use crate::seq::{Block, Event};
use crate::seq::{Pattern, Slot};
use crate::SAMPLE_RATE;
use crate::{app::AppCommand, sampler::SamplerCommand};
use anyhow::Result;
use ringbuf::{Consumer, Producer};
use std::sync::{
    atomic::{AtomicBool, AtomicU16, Ordering},
    Arc,
};

const MAX_INSTRUMENTS: usize = 32;
const COLUMNS_PER_TRACK: usize = 8;
const COLUMNS_PER_LINE: usize = COLUMNS_PER_TRACK * MAX_INSTRUMENTS;

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub enum HostParam {
    Bpm,
    Octave,
    LinesPerBeat,
}

pub enum HostCommand {
    PutInstrument(usize, Box<dyn Instrument>),
    PutPatternEvent { event: Event, slot: Slot },
    Device(usize, DeviceCommand),
}

pub trait Instrument {
    fn send_event(&mut self, column: usize, event: &Event);
    fn render(&mut self, buffer: &mut [(f32, f32)]);
    fn exec_command(&mut self, cmd: DeviceCommand) -> Result<()>;
}

pub enum DeviceCommand {
    Sampler(SamplerCommand),
}
#[derive(Clone)]
pub struct HostParams {
    pub bpm: Arc<AtomicU16>,
    pub lines_per_beat: Arc<AtomicU16>,
    pub octave: Arc<AtomicU16>,
    pub is_playing: Arc<AtomicBool>,
}

impl Default for HostParams {
    fn default() -> Self {
        Self {
            bpm: Arc::new(AtomicU16::new(120)),
            octave: Arc::new(AtomicU16::new(4)),
            lines_per_beat: Arc::new(AtomicU16::new(4)),
            is_playing: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl HostParams {
    pub fn set(&mut self, key: HostParam, value: u16) {
        match key {
            HostParam::Bpm => self.bpm.store(value, Ordering::Relaxed),
            HostParam::Octave => self.octave.store(value, Ordering::Relaxed),
            HostParam::LinesPerBeat => self.lines_per_beat.store(value, Ordering::Relaxed),
        }
    }

    pub fn get(&self, key: HostParam) -> u16 {
        match key {
            HostParam::Bpm => self.bpm.load(Ordering::Relaxed),
            HostParam::Octave => self.octave.load(Ordering::Relaxed),
            HostParam::LinesPerBeat => self.lines_per_beat.load(Ordering::Relaxed),
        }
    }
}

pub struct Host {
    cons: Consumer<HostCommand>,
    prod: Producer<AppCommand>,

    track_mapping: Vec<Option<Box<dyn Instrument>>>,
    current_pattern: Pattern,
    params: HostParams,

    samples_to_next_pulse: usize,
    current_pulse: u64,
}

impl Host {
    pub fn new(
        params: HostParams,
        cons: Consumer<HostCommand>,
        prod: Producer<AppCommand>,
    ) -> Host {
        Self {
            cons,
            prod,
            track_mapping: Vec::with_capacity(32),
            current_pattern: Pattern::new(),
            params,
            samples_to_next_pulse: 0,
            current_pulse: 0,
        }
    }

    pub fn render(&mut self, buffer: &mut [(f32, f32)]) {
        self.run_commands();
        let mut block = Block { start: 0, end: 0 };
        while self.next_block(&mut block, buffer.len()) {
            for instr in &mut self.track_mapping {
                if let Some(instr) = instr {
                    instr.render(&mut buffer[block.start..block.end]);
                }
            }
        }
    }

    pub fn run_commands(&mut self) {
        while let Some(update) = self.cons.pop() {
            match update {
                HostCommand::PutInstrument(_, instrument) => {
                    self.track_mapping.push(Some(instrument));
                }
                HostCommand::Device(index, cmd) => match cmd {
                    DeviceCommand::Sampler(_) => {
                        if let Some(instr) = &mut self.track_mapping[index] {
                            instr.exec_command(cmd).expect("exec command");
                        }
                    }
                },
                HostCommand::PutPatternEvent { event, slot } => {
                    self.current_pattern.add_event(event, slot);
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

        self.samples_to_next_pulse -= block.end - block.start;
        if block.end == num_frames {
            return false;
        }
        if block.end != 0 {
            block.start = block.end;
        }

        let pattern = &self.current_pattern;
        if self.samples_to_next_pulse == 0 {
            let line = self.current_pulse % pattern.num_lines as u64;
            let start = line as usize * COLUMNS_PER_LINE;
            let end = start + COLUMNS_PER_LINE;
            for column in start..end {
                let event = &pattern.events[column];
                if let Event::Empty = event {
                    continue;
                }
                let index = (column - start) / COLUMNS_PER_TRACK;
                let lane = (column - start) % COLUMNS_PER_TRACK;
                if let Some(Some(instrument)) = self.track_mapping.get_mut(index) {
                    instrument.send_event(lane, event);
                }
            }
            self.current_pulse += 1;
            let bpm = self.params.get(HostParam::Bpm);
            let lines_per_beat = self.params.get(HostParam::LinesPerBeat);
            let num_samples = (SAMPLE_RATE * 60.) / (lines_per_beat * bpm) as f64;
            self.samples_to_next_pulse = num_samples.round() as usize;
            self.set_current_line(line as usize);
        }

        block.end = block.start + self.samples_to_next_pulse;
        if block.end > num_frames {
            block.end = num_frames;
        }
        true
    }

    pub fn set_current_line(&mut self, line: usize) {
        self.app_send(AppCommand::SetCurrentLine(line));
    }

    fn app_send(&mut self, cmd: AppCommand) {
        if let Err(_) = self.prod.push(cmd) {
            eprintln!("unable to update client state");
        }
    }
}
