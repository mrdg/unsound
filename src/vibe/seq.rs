use super::{Event, EventType, Instrument};
use super::{BPM, PPQN, SAMPLE_RATE};

#[derive(Debug)]
struct Note {
    pos: i64,
    pitch: i32,
}

#[derive(Debug)]
pub struct Block {
    pub start: usize,
    pub end: usize,
}

pub struct Pattern {
    instrument: usize,
    length: i32,
    events: Vec<Event>,
}

impl Pattern {
    pub fn new(instrument: usize, notes: Vec<i32>) -> Pattern {
        // TODO: don't assume pattern length and step size
        let length = 4 * PPQN;
        let step_size = (PPQN / 4) as i32;

        let mut events = Vec::<Event>::new();
        for (step, pitch) in notes.iter().enumerate() {
            if *pitch == 0 {
                continue;
            }
            let pos = step as i32 * step_size;
            events.push(Event {
                pos: pos,
                r#type: EventType::NoteOn { pitch: *pitch },
            });
            events.push(Event {
                pos: pos + step_size,
                r#type: EventType::NoteOff { pitch: *pitch },
            });
        }
        Pattern {
            instrument,
            length,
            events,
        }
    }
}

pub struct Sequencer {
    samples_to_next_pulse: usize,
    current_pulse: i64,
    patterns: Vec<Pattern>,
}

impl Sequencer {
    pub fn new(patterns: Vec<Pattern>) -> Sequencer {
        Sequencer {
            samples_to_next_pulse: 0,
            current_pulse: 0,
            patterns,
        }
    }

    pub fn next_block(
        &mut self,
        block: &mut Block,
        num_frames: usize,
        instruments: &mut Vec<Box<dyn Instrument>>,
    ) -> bool {
        self.samples_to_next_pulse -= block.end - block.start;
        if block.end == num_frames {
            return false;
        }
        if block.end != 0 {
            block.start = block.end;
        }
        if self.samples_to_next_pulse == 0 {
            for pattern in &self.patterns {
                let pulse = self.current_pulse % pattern.length as i64;
                let instrument = &mut instruments[pattern.instrument];
                for event in &pattern.events {
                    if event.pos as i64 == pulse {
                        instrument.send_event(event);
                    }
                }
            }
            self.current_pulse += 1;
            let num_samples = (SAMPLE_RATE * 60.) / (BPM * PPQN) as f64;
            self.samples_to_next_pulse = num_samples.round() as usize;
        }
        block.end = block.start + self.samples_to_next_pulse;
        if block.end > num_frames {
            block.end = num_frames;
        }
        true
    }
}
