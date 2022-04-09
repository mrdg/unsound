use crate::audio::{Frame, Stereo};
use crate::engine::{Device, TrackContext};
use crate::env::{Envelope, State as EnvelopeState};
use crate::pattern::NoteEvent;
use crate::SAMPLE_RATE;
use anyhow::Result;
use camino::Utf8PathBuf;
use hound::WavReader;
use std::sync::Arc;

pub const ROOT_PITCH: u8 = 48;

struct Voice {
    position: f32,
    state: VoiceState,
    pitch_ratio: f32,
    pitch: u8,
    volume: f32,
    env: Envelope,
    column: usize,
    // TODO: it's possible that a voice ends up holding the last reference to a sound, which will
    // cause a deallocation on the audio thread.
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
    pub path: Utf8PathBuf,
    buf: Vec<Stereo>,
    sample_rate: u32,
    offset: usize,
}

pub fn load_sound(path: Utf8PathBuf) -> Result<Sound> {
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
    Ok(Sound {
        path,
        sample_rate: wav_spec.sample_rate,
        buf: samples,
        offset,
    })
}

pub struct Sampler {
    voices: Vec<Voice>,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
}

impl Sampler {
    pub fn new() -> Self {
        let num_voices = 8;
        let mut voices = Vec::with_capacity(num_voices);
        for _ in 0..num_voices {
            voices.push(Voice::new());
        }
        Self {
            attack: 0.005,
            decay: 0.25,
            sustain: 1.0,
            release: 0.3,
            voices,
        }
    }

    pub fn note_on(&mut self, sound: Arc<Sound>, column: usize, pitch: u8, velocity: u8) {
        self.stop_note(column);

        if let Some(voice) = self.voices.iter_mut().find(|v| v.state == VoiceState::Free) {
            voice.env.attack = self.attack;
            voice.env.decay = self.decay;
            voice.env.sustain = self.sustain;
            voice.env.release = self.release;
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
    fn render(&mut self, _ctx: TrackContext, buffer: &mut [Stereo]) {
        for voice in &mut self.voices {
            if voice.env.state == EnvelopeState::Init {
                voice.state = VoiceState::Free;
                voice.sound = None;
            }
            if voice.state != VoiceState::Busy {
                continue;
            }
            let sound = &voice.sound.as_ref().unwrap();
            for dst_frame in buffer.iter_mut() {
                let pos = voice.position as usize;
                let weight = voice.position - pos as f32;
                let inverse_weight = 1.0 - weight;

                let frame = sound.buf[pos];
                let next_frame = sound.buf[pos + 1];
                let new_frame = frame * inverse_weight + next_frame * weight;

                let env = voice.env.value() as f32;
                *dst_frame += new_frame * voice.volume * env;
                voice.position += voice.pitch_ratio;
                if voice.position >= (sound.buf.len() - 1) as f32 {
                    voice.state = VoiceState::Free;
                    voice.sound = None;
                    break;
                }
            }
        }
    }

    fn send_event(&mut self, ctx: TrackContext, event: &NoteEvent) {
        if let Some(snd) = ctx.sound(event.sound.into()) {
            self.note_on(snd.to_owned(), event.track as usize, event.pitch, 100);
        }
    }
}

fn map(v: f32, from: (f32, f32), to: (f32, f32)) -> f32 {
    (v - from.0) * (to.1 - to.0) / (from.1 - from.0) + to.0
}
