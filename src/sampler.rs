use crate::audio::{Buffer, Frame, Stereo};
use crate::engine::{Device, TrackContext};
use crate::env::{Envelope, State as EnvelopeState};
use crate::pattern::{NoteEvent, NOTE_OFF};
use crate::SAMPLE_RATE;
use anyhow::Result;
use camino::Utf8PathBuf;
use hound::WavReader;
use std::sync::Arc;
use std::time::Duration;

pub const ROOT_PITCH: u8 = 48;
const DEFAULT_RELEASE: Duration = Duration::from_millis(50);

struct Voice {
    position: f32,
    state: VoiceState,
    pitch_ratio: f32,
    pitch: u8,
    volume: f32,
    env: Envelope,
    column: usize,
    sample: Option<Arc<AudioFile>>,
    gate: f32,
}

#[derive(PartialEq, Debug)]
enum VoiceState {
    Free,
    Busy,
}

impl<'a> Voice {
    fn new() -> Self {
        let adsr = Adsr::default();
        Self {
            position: 0.0,
            column: 0,
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
    pub attack: Duration,
    pub decay: Duration,
    pub sustain: f32,
    pub release: Duration,
}

impl Adsr {
    pub fn set_attack(&mut self, secs: f32) {
        if secs >= 0.0 && secs <= 20.0 {
            self.attack = Duration::from_secs_f32(secs)
        }
    }

    pub fn set_decay(&mut self, secs: f32) {
        if secs >= 0.001 && secs <= 60.0 {
            self.decay = Duration::from_secs_f32(secs)
        }
    }

    pub fn set_sustain(&mut self, sustain: f32) {
        if sustain >= 0.005 && sustain <= 1.0 {
            self.sustain = sustain
        }
    }

    pub fn set_release(&mut self, secs: f32) {
        if secs >= 0.001 && secs <= 60.0 {
            self.release = Duration::from_secs_f32(secs)
        }
    }
}

impl Default for Adsr {
    fn default() -> Adsr {
        Self {
            attack: Duration::from_millis(0),
            decay: Duration::from_millis(50),
            sustain: 0.5,
            release: DEFAULT_RELEASE,
        }
    }
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
    pub fn new() -> Self {
        let mut voices = Vec::with_capacity(8);
        for _ in 0..voices.capacity() {
            voices.push(Voice::new());
        }
        Self { voices }
    }

    pub fn note_on(
        &mut self,
        sound: &Sound,
        ctx: TrackContext,
        column: usize,
        pitch: u8,
        velocity: u8,
    ) {
        self.note_off(column);

        if let Some(voice) = self.voices.iter_mut().find(|v| v.state == VoiceState::Free) {
            voice.gate = 1.0;
            voice.state = VoiceState::Busy;

            if let Some(adsr) = ctx.adsr {
                voice.env.update(adsr);
            }

            voice.pitch = pitch;
            voice.volume = gain_factor(map(velocity.into(), (0.0, 127.0), (-60.0, 0.0)));
            voice.column = column;

            let pitch = pitch as i8 - ROOT_PITCH as i8;
            voice.pitch_ratio = f32::powf(2., pitch as f32 / 12.0)
                * (sound.file.sample_rate as f32 / SAMPLE_RATE as f32);
            voice.position = sound.offset as f32;
            voice.sample = Some(sound.file.clone());
        } else {
            eprintln!("dropped event");
        }
    }

    fn note_off(&mut self, column: usize) {
        if let Some(voice) = self
            .voices
            .iter_mut()
            .find(|v| v.state == VoiceState::Busy && v.column == column)
        {
            voice.gate = 0.0;
        }
    }
}

fn gain_factor(db: f32) -> f32 {
    f32::powf(10.0, db / 20.0)
}

impl Device for Sampler {
    fn render(&mut self, ctx: TrackContext, buffer: &mut [Stereo]) {
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

                let mut env = 1.0;
                if let Some(adsr) = ctx.adsr {
                    voice.env.update(adsr);
                    env = voice.env.value(voice.gate) as f32;
                }
                *dst_frame += new_frame * voice.volume * env;

                voice.position += voice.pitch_ratio;
                if voice.position >= (sample.buf.len() - 1) as f32 {
                    voice.state = VoiceState::Free;
                    voice.sample = None;
                    break;
                }
            }
            if ctx.adsr.is_some() && voice.env.state == EnvelopeState::Idle {
                voice.state = VoiceState::Free;
                voice.sample = None;
            }
        }
    }

    fn send_event(&mut self, ctx: TrackContext, event: &NoteEvent) {
        if event.pitch == NOTE_OFF {
            self.note_off(event.track as usize);
        } else if let Some(sound) = ctx.sound(event.sound.into()) {
            self.note_on(sound, ctx, event.track.into(), event.pitch, 100);
        }
    }
}

fn map(v: f32, from: (f32, f32), to: (f32, f32)) -> f32 {
    (v - from.0) * (to.1 - to.0) / (from.1 - from.0) + to.0
}
