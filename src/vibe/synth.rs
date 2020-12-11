use super::{env, Event, EventType, Instrument, SAMPLE_RATE};
use std::f64::consts::PI;

const TWO_PI: f64 = 2. * PI;

pub struct Synth {
    voices: Vec<Voice>,
}

impl Instrument for Synth {
    fn send_event(&mut self, event: &Event) {
        match event.r#type {
            EventType::NoteOn { pitch } => self.voice_note_on(pitch),
            EventType::NoteOff { pitch } => {
                self.voice_note_off(pitch);
            }
        }
    }

    fn render(&mut self, buffer: &mut [f32]) {
        for voice in &mut self.voices {
            voice.render(buffer);
        }
    }
}

impl Synth {
    pub fn new() -> Synth {
        Synth {
            voices: vec![Voice::new(), Voice::new(), Voice::new(), Voice::new()],
        }
    }

    fn voice_note_on(&mut self, pitch: i32) {
        for voice in &mut self.voices {
            match voice.state {
                VoiceState::Free => {
                    voice.state = VoiceState::Busy;
                    voice.note_on(pitch);
                    return;
                }
                VoiceState::Busy => continue,
            }
        }
        println!("no free voice found");
    }

    fn voice_note_off(&mut self, pitch: i32) {
        for voice in &mut self.voices {
            match voice.state {
                VoiceState::Busy if voice.pitch == pitch => voice.note_off(),
                _ => continue,
            }
        }
    }
}

#[derive(PartialEq)]
enum VoiceState {
    Free,
    Busy,
}

struct Voice {
    pitch: i32,
    osc: Osc,
    env: env::Envelope,
    state: VoiceState,
}

impl Voice {
    fn new() -> Voice {
        Voice {
            pitch: 0,
            osc: Osc {
                phase: 0.0,
                phase_delta: 0.0,
            },
            env: env::Envelope::new(),
            state: VoiceState::Free,
        }
    }

    fn render(&mut self, buffer: &mut [f32]) {
        for i in 0..buffer.len() {
            if self.env.state == env::State::Init {
                self.state = VoiceState::Free;
            }
            if self.state == VoiceState::Free {
                buffer[i] += 0.0;
                continue;
            }
            self.osc.phase += self.osc.phase_delta;
            let sample = (2. * self.osc.phase) / TWO_PI - 1.;
            if self.osc.phase >= TWO_PI {
                self.osc.phase -= TWO_PI
            }
            buffer[i] += (sample * self.env.value() * 0.1) as f32;
        }
    }

    fn note_on(&mut self, pitch: i32) {
        let freq = midi_to_freq(pitch as f64);
        self.pitch = pitch;
        self.osc.phase_delta = freq * TWO_PI / SAMPLE_RATE;
        self.osc.phase = 0.0;
        self.env.start_attack();
    }

    fn note_off(&mut self) {
        self.pitch = 0;
        self.env.start_release();
    }
}

struct Osc {
    phase_delta: f64,
    phase: f64,
}

fn midi_to_freq(note: f64) -> f64 {
    f64::powf(2.0, (note - 69.0) / 12.0) * 440.0
}
