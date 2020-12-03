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

pub struct Sequencer {
    samples_to_next_pulse: usize,
    current_pulse: i64,
    pattern_length: i64,
    events: Vec<Event>,
}

impl Sequencer {
    pub fn new() -> Sequencer {
        Sequencer {
            samples_to_next_pulse: 0,
            current_pulse: 0,
            pattern_length: 4 * PPQN as i64,
            events: Vec::new(),
        }
    }

    pub fn add_note(&mut self, pitch: i32, pos: i32, duration: i32) {
        self.events.push(Event {
            pos: pos,
            r#type: EventType::NoteOn { pitch },
        });
        self.events.push(Event {
            pos: pos + duration,
            r#type: EventType::NoteOff { pitch },
        });
    }

    pub fn next_block(
        &mut self,
        block: &mut Block,
        num_frames: usize,
        instrument: &mut dyn Instrument,
    ) -> bool {
        self.samples_to_next_pulse -= block.end - block.start;
        if block.end == num_frames {
            return false;
        }
        if block.end != 0 {
            block.start = block.end;
        }
        if self.samples_to_next_pulse == 0 {
            let pulse = self.current_pulse % self.pattern_length;
            for event in &self.events {
                if event.pos as i64 == pulse {
                    instrument.send_event(event);
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
