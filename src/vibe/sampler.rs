use super::{Event, EventType, Instrument};
use std::cmp;

pub struct Sound {
    samples: Vec<f32>,
    position: usize,
    state: VoiceState,
    pub pitch: i32,
}

#[derive(PartialEq)]
enum VoiceState {
    Free,
    Busy,
}

impl Sound {
    pub fn load(path: String, pitch: i32) -> Sound {
        // TODO: handle errors instead of panicking
        let mut wav = hound::WavReader::open(path.clone()).unwrap();
        let bit_depth = wav.spec().bits_per_sample as f32;
        let samples: Vec<f32> = wav
            .samples::<i32>()
            .map(|sample| sample.unwrap() as f32 / (f32::powf(2., bit_depth - 1.)))
            .collect();

        Sound {
            samples: samples,
            pitch: pitch,
            position: 0,
            state: VoiceState::Free,
        }
    }
}

pub struct Sampler {
    sounds: Vec<Sound>,
}

impl Sampler {
    pub fn new(sounds: Vec<Sound>) -> Sampler {
        Sampler { sounds }
    }

    fn note_on(&mut self, pitch: i32) {
        for snd in &mut self.sounds {
            match snd.state {
                VoiceState::Free if snd.pitch == pitch => {
                    snd.state = VoiceState::Busy;
                    return;
                }
                _ => continue,
            }
        }
    }
}

impl Instrument for Sampler {
    fn send_event(&mut self, event: &Event) {
        match event.r#type {
            EventType::NoteOn { pitch } => {
                self.note_on(pitch);
            }
            _ => return,
        }
    }

    fn render(&mut self, buffer: &mut [f32]) {
        for sound in &mut self.sounds {
            if sound.state != VoiceState::Busy {
                continue;
            }
            let samples_left = sound.samples.len() - sound.position;
            let len = cmp::min(buffer.len(), samples_left);
            for i in 0..len {
                buffer[i] = sound.samples[sound.position];
                sound.position += 1;
            }
            if sound.position == sound.samples.len() {
                sound.state = VoiceState::Free;
                sound.position = 0;
            }
        }
    }
}
