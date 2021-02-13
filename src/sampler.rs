use crate::engine::Device;
use crate::param::Param;
use crate::SAMPLE_RATE;
use crate::{
    env::{Envelope, State as EnvelopeState},
    param::Unit,
};
use anyhow::Result;
use atomic_float::AtomicF32;
use camino::Utf8PathBuf;
use hound::WavReader;
use std::ops::{Add, Mul};
use std::sync::{atomic::Ordering, Arc};

pub const ROOT_PITCH: u8 = 48;

struct Voice {
    position: f32,
    state: VoiceState,
    pitch_ratio: f32,
    pitch: u8,
    volume: f32,
    env: Envelope,
    column: usize,
    sound: Option<Arc<Sound>>,
}

#[derive(PartialEq, Debug)]
enum VoiceState {
    Free,
    Busy,
}

impl<'a> Voice {
    fn new() -> Self {
        Self {
            position: 0.0,
            column: 0,
            pitch: 0,
            volume: 0.0,
            pitch_ratio: 0.,
            state: VoiceState::Free,
            env: Envelope::new(),
            sound: None,
        }
    }
}

pub struct Sound {
    buf: Vec<Frame>,
    sample_rate: u32,
    offset: usize,
}

pub struct Sampler {
    voices: Vec<Voice>,
    amp: Arc<AtomicF32>,
    attack: Arc<AtomicF32>,
    decay: Arc<AtomicF32>,
    sustain: Arc<AtomicF32>,
    release: Arc<AtomicF32>,
}

impl Sampler {
    pub fn new() -> Self {
        let num_voices = 8;
        let mut voices = Vec::with_capacity(num_voices);
        for _ in 0..num_voices {
            voices.push(Voice::new());
        }
        Self {
            amp: Arc::new(AtomicF32::new(-6.0)),
            attack: Arc::new(AtomicF32::new(0.005)),
            decay: Arc::new(AtomicF32::new(0.25)),
            sustain: Arc::new(AtomicF32::new(1.0)),
            release: Arc::new(AtomicF32::new(0.3)),
            voices,
        }
    }

    pub fn load_sound(path: &Utf8PathBuf) -> Result<Sound> {
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
                break;
            }
        }
        Ok(Sound {
            sample_rate: wav_spec.sample_rate,
            buf: samples,
            offset,
        })
    }

    pub fn params(&self) -> Vec<(String, Param)> {
        vec![
            (
                "Amp",
                Param::new(-60.0, Arc::clone(&self.amp), 6.0, 1.0).with_unit(Unit::Decibel),
            ),
            (
                "Attack",
                Param::new(0.0, Arc::clone(&self.attack), 15.0, 0.01).with_unit(Unit::Seconds),
            ),
            (
                "Decay",
                Param::new(0.0, Arc::clone(&self.decay), 15.0, 0.01).with_unit(Unit::Seconds),
            ),
            (
                "Sustain",
                Param::new(0.0, Arc::clone(&self.sustain), 15.0, 0.01),
            ),
            (
                "Release",
                Param::new(0.0, Arc::clone(&self.release), 15.0, 0.01).with_unit(Unit::Seconds),
            ),
        ]
        .into_iter()
        .map(|(k, v)| (String::from(k), v))
        .collect()
    }

    pub fn note_on(&mut self, sound: Arc<Sound>, column: usize, pitch: u8, velocity: u8) {
        self.stop_note(column);

        let attack = self.attack.load(Ordering::Relaxed);
        let decay = self.decay.load(Ordering::Relaxed);
        let sustain = self.sustain.load(Ordering::Relaxed);
        let release = self.release.load(Ordering::Relaxed);
        if let Some(voice) = self.voices.iter_mut().find(|v| v.state == VoiceState::Free) {
            voice.env.attack = attack;
            voice.env.decay = decay;
            voice.env.sustain = sustain;
            voice.env.release = release;
            voice.env.start_attack();
            voice.state = VoiceState::Busy;
            voice.pitch = pitch;
            voice.volume = gain_factor(map(velocity as f32, (0.0, 127.0), (-60.0, 0.0)));
            voice.column = column;
            let pitch = pitch as i8 - ROOT_PITCH as i8;
            voice.pitch_ratio = f32::powf(2., pitch as f32 / 12.0)
                * (sound.sample_rate as f32 / SAMPLE_RATE as f32);
            voice.position = sound.offset as f32;
            voice.sound = Some(sound);
        } else {
            eprintln!("dropped event");
        }
    }

    fn note_off(&mut self, column: usize, pitch: u8) {
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

impl Device for Sampler {
    fn render(&mut self, buffer: &mut [(f32, f32)]) {
        let amp = gain_factor(self.amp.load(Ordering::Relaxed));

        for voice in &mut self.voices {
            if voice.env.state == EnvelopeState::Init {
                voice.state = VoiceState::Free;
                voice.sound = None;
            }
            if voice.state != VoiceState::Busy {
                continue;
            }
            let sound = &voice.sound.as_ref().unwrap();
            for i in 0..buffer.len() {
                let pos = voice.position as usize;
                let weight = voice.position - pos as f32;
                let inverse_weight = 1.0 - weight;

                let frame = &sound.buf[pos];
                let next_frame = &sound.buf[pos + 1];
                let new_frame = frame * inverse_weight + next_frame * weight;

                let env = voice.env.value() as f32;
                buffer[i].0 += voice.volume * amp * env * new_frame.left;
                buffer[i].1 += voice.volume * amp * env * new_frame.right;
                voice.position += voice.pitch_ratio;
                if voice.position >= (sound.buf.len() - 1) as f32 {
                    voice.state = VoiceState::Free;
                    voice.sound = None;
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

fn map(v: f32, from: (f32, f32), to: (f32, f32)) -> f32 {
    (v - from.0) * (to.1 - to.0) / (from.1 - from.0) + to.0
}
