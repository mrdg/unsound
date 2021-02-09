use crate::host::{DeviceCommand, Instrument};
use crate::param::Param;
use crate::seq::Event;
use crate::SAMPLE_RATE;
use crate::{
    env::{Envelope, State as EnvelopeState},
    param::Unit,
};
use anyhow::Result;
use atomic_float::AtomicF32;
use hound::WavReader;
use std::{
    ops::{Add, Mul},
    path::PathBuf,
};
use std::{
    rc::Rc,
    sync::{atomic::Ordering, Arc},
};

const ROOT_PITCH: i32 = 48;

pub enum SamplerCommand {
    LoadSound(Sound),
}

struct Voice {
    position: f32,
    state: VoiceState,
    pitch_ratio: f32,
    pitch: i32,
    env: Envelope,
    column: usize,
    sound: Rc<Sound>,
}

#[derive(PartialEq, Debug)]
enum VoiceState {
    Free,
    Busy,
}

impl<'a> Voice {
    fn new(sound: Rc<Sound>) -> Self {
        Self {
            position: 0.0,
            column: 0,
            pitch: 0,
            pitch_ratio: 0.,
            state: VoiceState::Free,
            env: Envelope::new(),
            sound,
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
    cued_sound: Option<Rc<Sound>>,
    amp: Arc<AtomicF32>,
    offset: Arc<AtomicF32>,
    attack: Arc<AtomicF32>,
    decay: Arc<AtomicF32>,
    sustain: Arc<AtomicF32>,
    release: Arc<AtomicF32>,
}

impl Sampler {
    pub fn with_sample(sound: Sound) -> Result<Sampler> {
        let num_voices = 8;
        let sound = Rc::new(sound);
        let mut voices = Vec::with_capacity(num_voices);
        for _ in 0..num_voices {
            voices.push(Voice::new(sound.clone()));
        }
        Ok(Sampler {
            amp: Arc::new(AtomicF32::new(-6.0)),
            offset: Arc::new(AtomicF32::new(sound.offset as f32)),
            attack: Arc::new(AtomicF32::new(0.005)),
            decay: Arc::new(AtomicF32::new(0.25)),
            sustain: Arc::new(AtomicF32::new(0.0)),
            release: Arc::new(AtomicF32::new(0.0)),
            cued_sound: None,
            voices,
        })
    }

    pub fn load_sound(path: &PathBuf) -> Result<Sound> {
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
                "Offset",
                Param::new(0.0, Arc::clone(&self.offset), f32::MAX, 1.0),
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
                Param::new(0.0, Arc::clone(&self.sustain), 15.0, 0.01).with_unit(Unit::Seconds),
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

    fn note_on(&mut self, column: usize, pitch: i32) {
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
            voice.column = column;
            voice.pitch_ratio = f32::powf(2., (pitch - ROOT_PITCH) as f32 / 12.0)
                * (voice.sound.sample_rate as f32 / SAMPLE_RATE as f32);
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
    fn exec_command(&mut self, cmd: DeviceCommand) -> Result<()> {
        match cmd {
            DeviceCommand::Sampler(cmd) => match cmd {
                SamplerCommand::LoadSound(snd) => {
                    self.offset.store(snd.offset as f32, Ordering::Relaxed);
                    self.cued_sound = Some(Rc::new(snd));
                }
            },
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
        let amp = gain_factor(self.amp.load(Ordering::Relaxed));

        let offset = self.offset.load(Ordering::Relaxed);
        for voice in &mut self.voices {
            if voice.env.state == EnvelopeState::Init {
                voice.state = VoiceState::Free;
                voice.position = offset;
                if let Some(snd) = &self.cued_sound {
                    voice.sound = snd.clone();
                }
            }
            if voice.state != VoiceState::Busy {
                continue;
            }
            for i in 0..buffer.len() {
                let pos = voice.position as usize;
                let weight = voice.position - pos as f32;
                let inverse_weight = 1.0 - weight;

                let frame = &voice.sound.buf[pos];
                let next_frame = &voice.sound.buf[pos + 1];
                let new_frame = frame * inverse_weight + next_frame * weight;

                let env = voice.env.value() as f32;
                buffer[i].0 += amp * env * new_frame.left;
                buffer[i].1 += amp * env * new_frame.right;
                voice.position += voice.pitch_ratio;
                if voice.position >= (voice.sound.buf.len() - 1) as f32 {
                    voice.state = VoiceState::Free;
                    voice.position = offset;
                    if let Some(snd) = &self.cued_sound {
                        voice.sound = snd.clone();
                    }
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
