pub mod env;
pub mod seq;
pub mod synth;

pub const SAMPLE_RATE: f64 = 44_100.0;
pub const FRAMES_PER_BUFFER: u32 = 64;
pub const PPQN: i64 = 960;
const BPM: i64 = 120;

pub struct Event {
    pos: i32,
    r#type: EventType,
}
pub enum EventType {
    NoteOn { pitch: i32 },
    NoteOff { pitch: i32 },
}

pub trait Instrument {
    fn send_event(&mut self, event: &Event);
}
