use crate::env::{Envelope, State as EnvelopeState};
use crate::param::{Param, ParamKey, Unit};
use crate::seq::{Event, Instrument};
use crate::SAMPLE_RATE;
use hound::{WavReader, WavSpec};
use std::error::Error;
use std::ops::{Add, Mul};

const ROOT_PITCH: i32 = 48;

#[derive(Debug)]
struct Voice {
    position: f32,
    state: VoiceState,
    pitch_ratio: f32,
    pitch: i32,
    env: Envelope,
    column: usize,
}

#[derive(PartialEq, Debug)]
enum VoiceState {
    Free,
    Busy,
}

impl Voice {
    fn new() -> Self {
        Self {
            position: 0.0,
            column: 0,
            pitch: 0,
            pitch_ratio: 0.,
            state: VoiceState::Free,
            env: Envelope::new(),
        }
    }
}

pub struct Sampler {
    voices: Vec<Voice>,
    samples: Vec<Frame>,
    sample_rate: u32,
    amp: f32,
    amp_env_attack: f32,
    amp_env_decay: f32,
    amp_env_sustain: f32,
    amp_env_release: f32,
    sample_offset: f32,
}

impl Sampler {
    pub fn with_sample(path: &str) -> Result<Sampler, Box<dyn Error>> {
        let num_voices = 8;
        let mut voices = Vec::with_capacity(num_voices);
        for _ in 0..num_voices {
            voices.push(Voice::new());
        }
        let (wav_spec, samples, offset) = Self::load_sound(String::from(path))?;
        let sampler = Sampler {
            sample_rate: wav_spec.sample_rate,
            voices,
            samples,
            amp: -6.0,
            amp_env_attack: 0.005,
            amp_env_decay: 0.5,
            amp_env_sustain: 0.0,
            amp_env_release: 0.0,
            sample_offset: offset as f32,
        };
        Ok(sampler)
    }

    fn load_sound(path: String) -> Result<(WavSpec, Vec<Frame>, usize), Box<dyn Error>> {
        let mut wav = WavReader::open(path.clone())?;
        let wav_spec = wav.spec();
        let bit_depth = wav_spec.bits_per_sample as f32;
        let samples: Vec<Frame> = wav
            .samples::<i32>()
            .map(|sample| sample.unwrap() as f32 / (f32::powf(2., bit_depth - 1.)))
            .collect::<Vec<f32>>()
            .chunks(wav_spec.channels as usize)
            .map(|f| {
                let left = *f.get(0).unwrap();
                let right = *f.get(1).unwrap_or(&left);
                Frame { left, right }
            })
            .collect();

        const SILENCE: f32 = 0.01;
        let mut offset = 0;
        for (i, frame) in samples.iter().enumerate() {
            if frame.left < SILENCE && frame.right < SILENCE {
                continue;
            } else {
                offset = i;
                eprintln!("sample {} starts at {}", path, i);
                break;
            }
        }
        Ok((wav_spec, samples, offset))
    }

    pub fn params(&self) -> Vec<(ParamKey, Param)> {
        vec![
            (
                ParamKey::Amp,
                Param::new(-75.0, self.amp, 6.0, 1.0).with_unit(Unit::Decibel),
            ),
            (
                ParamKey::SampleOffset,
                Param::new(0.0, self.sample_offset as f32, f32::MAX, 1.0).with_unit(Unit::Samples),
            ),
            (
                ParamKey::AmpEnvAttack,
                Param::new(0.005, self.amp_env_attack, 15.0, 0.001).with_unit(Unit::Seconds),
            ),
            (
                ParamKey::AmpEnvDecay,
                Param::new(0.005, self.amp_env_decay, 15.0, 0.001).with_unit(Unit::Seconds),
            ),
            (
                ParamKey::AmpEnvSustain,
                Param::new(0.0, self.amp_env_sustain, 1.0, 0.01),
            ),
            (
                ParamKey::AmpEnvRelease,
                Param::new(0.005, self.amp_env_release, 15.0, 0.001).with_unit(Unit::Seconds),
            ),
        ]
    }

    fn note_on(&mut self, column: usize, pitch: i32) {
        if let Some(voice) = self.voices.iter_mut().find(|v| v.state == VoiceState::Free) {
            voice.env.attack = self.amp_env_attack;
            voice.env.decay = self.amp_env_decay;
            voice.env.sustain = self.amp_env_sustain;
            voice.env.release = self.amp_env_release;
            voice.env.start_attack();
            voice.state = VoiceState::Busy;
            voice.pitch = pitch;
            voice.column = column;
            voice.pitch_ratio = f32::powf(2., (pitch - ROOT_PITCH) as f32 / 12.0)
                * (self.sample_rate as f32 / SAMPLE_RATE as f32);
        } else {
            eprintln!("dropped event");
        }
    }

    fn note_off(&mut self, column: usize, pitch: i32) {
        if let Some(voice) = self
            .voices
            .iter_mut()
            .find(|v| v.state == VoiceState::Busy && v.column == column && v.pitch == pitch)
        {
            voice.env.start_release();
        }
    }

    fn stop_note(&mut self, column: usize) {
        if let Some(voice) = self
            .voices
            .iter_mut()
            .find(|v| v.state == VoiceState::Busy && v.column == column)
        {
            voice.env.release = 0.005; // set a short release (5ms)
            voice.env.start_release();
        }
    }
}

fn gain_factor(db: f32) -> f32 {
    f32::powf(10.0, db / 20.0)
}

impl Instrument for Sampler {
    fn set_param(&mut self, key: ParamKey, p: Param) -> Result<(), Box<dyn Error>> {
        match key {
            ParamKey::Amp => self.amp = p.val,
            ParamKey::AmpEnvAttack => self.amp_env_attack = p.val,
            ParamKey::AmpEnvDecay => self.amp_env_decay = p.val,
            ParamKey::AmpEnvSustain => self.amp_env_sustain = p.val,
            ParamKey::AmpEnvRelease => self.amp_env_release = p.val,
            ParamKey::SampleOffset => self.sample_offset = p.val,
        }
        Ok(())
    }

    fn send_event(&mut self, column: usize, event: &Event) {
        match event {
            Event::NoteOn { pitch } => {
                self.stop_note(column);
                self.note_on(column, *pitch);
            }
            Event::NoteOff { pitch } => {
                self.note_off(column, *pitch);
            }
            Event::Empty => {}
        }
    }

    fn render(&mut self, buffer: &mut [(f32, f32)]) {
        for voice in &mut self.voices {
            if voice.env.state == EnvelopeState::Init {
                voice.state = VoiceState::Free;
                voice.position = self.sample_offset;
            }
            if voice.state != VoiceState::Busy {
                continue;
            }
            for i in 0..buffer.len() {
                let pos = voice.position as usize;
                let weight = voice.position - pos as f32;
                let inverse_weight = 1.0 - weight;

                let frame = &self.samples[pos];
                let next_frame = &self.samples[pos + 1];
                let new_frame = frame * inverse_weight + next_frame * weight;

                let amp = gain_factor(self.amp);
                let env = voice.env.value() as f32;
                buffer[i].0 += amp * env * new_frame.left;
                buffer[i].1 += amp * env * new_frame.right;
                voice.position += voice.pitch_ratio;
                if voice.position >= (self.samples.len() - 1) as f32 {
                    voice.state = VoiceState::Free;
                    voice.position = self.sample_offset;
                    break;
                }
            }
        }
    }
}

struct Frame {
    left: f32,
    right: f32,
}

impl Mul<f32> for &Frame {
    type Output = Frame;

    fn mul(self, f: f32) -> Frame {
        Frame {
            left: self.left * f,
            right: self.right * f,
        }
    }
}

impl Add for Frame {
    type Output = Frame;

    fn add(self, other: Frame) -> Frame {
        Frame {
            left: self.left + other.left,
            right: self.right + other.right,
        }
    }
}
