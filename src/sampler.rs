use crate::audio::{Buffer, Frame, Stereo};
use crate::engine::{Device, DeviceContext};
use crate::env::{Envelope, State as EnvelopeState};
use crate::params::{self, ParamInfo, Params, ParamsBuilder};
use crate::pattern::{Effect, NoteEvent, DEFAULT_VELOCITY, NOTE_OFF};
use crate::SAMPLE_RATE;
use anyhow::Result;
use camino::Utf8PathBuf;
use hound::WavReader;
use std::sync::Arc;

pub const ROOT_PITCH: u8 = 48;

#[derive(Clone)]
enum SamplerParam {
    EnvAttack = 0,
    EnvDecay,
    EnvSustain,
    EnvRelease,
}

impl From<SamplerParam> for usize {
    fn from(p: SamplerParam) -> usize {
        p as usize
    }
}

struct Voice {
    position: f32,
    state: VoiceState,
    pitch_ratio: f32,
    pitch: u8,
    volume: f32,
    env: Envelope,
    sample: Option<Arc<AudioFile>>,
    gate: f64,
}

#[derive(PartialEq, Debug)]
enum VoiceState {
    Free,
    Busy,
}

impl<'a> Voice {
    fn new(params: &Params) -> Self {
        let adsr = params.adsr();
        Self {
            position: 0.0,
            pitch: 0,
            volume: 0.0,
            pitch_ratio: 0.,
            state: VoiceState::Free,
            env: Envelope::new(adsr.attack, adsr.decay, adsr.sustain, adsr.release),
            sample: None,
            gate: 0.0,
        }
    }
}

#[derive(Clone)]
pub struct Sound {
    pub path: Utf8PathBuf,
    pub offset: usize,
    pub file: Arc<AudioFile>,
}

#[derive(Clone)]
pub struct Adsr {
    pub attack: f64,
    pub decay: f64,
    pub sustain: f64,
    pub release: f64,
}

pub struct AudioFile {
    sample_rate: u32,
    buf: Buffer,
    pub offset: usize,
}

pub fn load_file(path: &Utf8PathBuf) -> Result<AudioFile> {
    let mut wav = WavReader::open(path.clone())?;
    let wav_spec = wav.spec();
    let bit_depth = wav_spec.bits_per_sample as f32;
    let samples: Vec<Stereo> = wav
        .samples::<i32>()
        .map(|sample| sample.unwrap() as f32 / (f32::powf(2., bit_depth - 1.)))
        .collect::<Vec<f32>>()
        .chunks(wav_spec.channels as usize)
        .map(|f| {
            let left = *f.get(0).unwrap();
            let right = *f.get(1).unwrap_or(&left);
            Frame::new([left, right])
        })
        .collect();

    const SILENCE: f32 = 0.01;
    let mut offset = 0;
    for (i, frame) in samples.iter().enumerate() {
        if frame.channel(0) < SILENCE && frame.channel(1) < SILENCE {
            continue;
        } else {
            offset = i;
            break;
        }
    }
    Ok(AudioFile {
        sample_rate: wav_spec.sample_rate,
        offset,
        buf: samples,
    })
}

pub struct Sampler {
    voices: Vec<Voice>,
}

impl Sampler {
    pub fn params() -> Params {
        let mut p = ParamsBuilder::new();
        use SamplerParam::*;
        p.insert(
            EnvAttack,
            ParamInfo::new("Envelope Attack", 1, 1, 20_000)
                .with_steps([5, 100])
                .with_formatter(params::format_millis),
        );
        p.insert(
            EnvDecay,
            ParamInfo::new("Envelope Decay", 200, 5, 20_000)
                .with_steps([5, 100])
                .with_formatter(params::format_millis),
        );
        p.insert(
            EnvSustain,
            ParamInfo::new("Envelope Sustain", 1.0, 0.01, 1.0),
        );
        p.insert(
            EnvRelease,
            ParamInfo::new("Envelope Release", 100, 5, 20_000)
                .with_steps([5, 100])
                .with_formatter(params::format_millis),
        );
        p.build()
    }

    pub fn new(params: &Params) -> Self {
        let mut voices = Vec::with_capacity(12);
        for _ in 0..voices.capacity() {
            voices.push(Voice::new(params));
        }
        Self { voices }
    }

    pub fn note_on(&mut self, sound: &Sound, ctx: DeviceContext, pitch: u8, velocity: u8) {
        if let Some(voice) = self.voices.iter_mut().find(|v| v.state == VoiceState::Free) {
            voice.gate = 1.0;
            voice.state = VoiceState::Busy;

            let adsr = ctx.params().adsr();
            voice.env = Envelope::new(adsr.attack, adsr.decay, adsr.sustain, adsr.release);

            voice.pitch = pitch;
            voice.volume = gain_factor(map(velocity.into(), (0.0, 127.0), (-60.0, 0.0)));

            let pitch = pitch as i8 - ROOT_PITCH as i8;
            voice.pitch_ratio = f32::powf(2., pitch as f32 / 12.0)
                * (sound.file.sample_rate as f32 / SAMPLE_RATE as f32);
            voice.position = sound.offset as f32;
            voice.sample = Some(sound.file.clone());
        } else {
            eprintln!("dropped event");
        }
    }

    fn release_voices(&mut self) {
        for v in &mut self.voices {
            if v.state == VoiceState::Busy {
                v.gate = 0.0;
            }
        }
    }
}

fn gain_factor(db: f32) -> f32 {
    f32::powf(10.0, db / 20.0)
}

trait SamplerConfig {
    fn adsr(&self) -> Adsr;
}

impl<'a> SamplerConfig for Params {
    fn adsr(&self) -> Adsr {
        use SamplerParam::*;
        Adsr {
            attack: self.value(EnvAttack),
            decay: self.value(EnvDecay),
            sustain: self.value(EnvSustain),
            release: self.value(EnvRelease),
        }
    }
}

impl Device for Sampler {
    fn render(&mut self, ctx: DeviceContext, buffer: &mut [Stereo]) {
        let adsr = ctx.params().adsr();
        for voice in &mut self.voices {
            if voice.state == VoiceState::Free {
                continue;
            }
            let sample = voice.sample.as_ref().unwrap();
            for dst_frame in buffer.iter_mut() {
                let pos = voice.position as usize;
                let weight = voice.position - pos as f32;
                let inverse_weight = 1.0 - weight;

                let frame = sample.buf[pos];
                let next_frame = sample.buf[pos + 1];
                let new_frame = frame * inverse_weight + next_frame * weight;

                voice.env.update(&adsr);
                *dst_frame += new_frame * voice.volume * voice.env.value(voice.gate) as f32;

                voice.position += voice.pitch_ratio;
                if voice.position >= (sample.buf.len() - 1) as f32 {
                    voice.state = VoiceState::Free;
                    voice.sample = None;
                    break;
                }
            }
            if voice.env.state == EnvelopeState::Idle {
                voice.state = VoiceState::Free;
                voice.sample = None;
            }
        }
    }

    fn send_event(&mut self, ctx: DeviceContext, event: &NoteEvent) {
        let mut velocity: Option<u8> = None;
        let mut chord: [Option<u8>; 3] = [None; 3];

        for effect in [event.fx1, event.fx2].iter().flatten() {
            match effect {
                Effect::Chord(c) => {
                    if let Some(c) = c {
                        chord = make_chord(*c);
                    }
                }
                Effect::Velocity(v) => {
                    if let Some(v) = v {
                        velocity = Some(*v);
                    }
                }
            }
        }
        if event.pitch == NOTE_OFF {
            self.release_voices();
        } else if let Some(sound) = ctx.sound(event.sound.into()) {
            self.release_voices();
            let velocity = velocity.unwrap_or(DEFAULT_VELOCITY);
            self.note_on(sound, ctx, event.pitch, velocity);
            for offset in chord.iter().flatten() {
                self.note_on(sound, ctx, event.pitch + offset, velocity);
            }
        }
    }
}

fn map(v: f32, from: (f32, f32), to: (f32, f32)) -> f32 {
    (v - from.0) * (to.1 - to.0) / (from.1 - from.0) + to.0
}

fn make_chord(n: i16) -> [Option<u8>; 3] {
    let mut n = n;
    let mut d = 100;
    let mut chord: [Option<u8>; 3] = [None; 3];
    for offset in &mut chord {
        let semitones = n / d;
        if semitones > 0 {
            *offset = Some(semitones as u8)
        }
        n %= d;
        d /= 10;
    }
    chord
}
