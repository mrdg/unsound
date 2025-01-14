use crate::app::TrackId;
use crate::audio::{Buffer, Frame, Stereo};
use crate::engine::{Event, Note, Plugin, ProcessContext, ProcessStatus};
use crate::env::{Envelope, State as EnvelopeState};
use crate::params::{self, format_millis, Param, ParamInfo, Params};
use crate::SAMPLE_RATE;
use anyhow::Result;
use camino::Utf8PathBuf;
use hound::{SampleFormat, WavReader};
use param_derive::Params;
use std::ops::Range;
use std::sync::Arc;

pub const ROOT_PITCH: u8 = 48;

#[derive(Params)]
pub struct SamplerParams {
    env_attack: Param,
    env_decay: Param,
    env_sustain: Param,
    env_release: Param,
}

impl SamplerParams {
    fn adsr(&self) -> Adsr {
        Adsr {
            attack: self.env_attack.value(),
            decay: self.env_decay.value(),
            sustain: self.env_sustain.value(),
            release: self.env_release.value(),
        }
    }
}
#[derive(Clone)]
pub struct Adsr {
    pub attack: f64,
    pub decay: f64,
    pub sustain: f64,
    pub release: f64,
}

impl Default for SamplerParams {
    fn default() -> Self {
        Self {
            env_attack: Param::new(
                1.0,
                ParamInfo::new("Envelope Attack", 1, 20_000)
                    .with_steps([5, 100])
                    .with_formatter(format_millis),
            ),
            env_decay: Param::new(
                200.0,
                ParamInfo::new("Envelope Decay", 5, 20_000)
                    .with_steps([5, 100])
                    .with_formatter(format_millis),
            ),
            env_sustain: Param::new(1.0, ParamInfo::new("Envelope Sustain", 0.01, 1.0)),
            env_release: Param::new(
                100.0,
                ParamInfo::new("Envelope Release", 5, 20_000)
                    .with_steps([5, 100])
                    .with_formatter(format_millis),
            ),
        }
    }
}

pub struct Voice {
    params: Arc<SamplerParams>,
    position: f32,
    state: VoiceState,
    pitch_ratio: f32,
    pitch: u8,
    velocity: f32,
    env: Envelope,
    sample: Arc<Buffer>,
    gate: f64,
}

#[derive(PartialEq, Eq, Debug)]
pub enum VoiceState {
    Free,
    Busy(TrackId),
}

impl Voice {
    fn new(params: Arc<SamplerParams>, sample: Arc<Buffer>) -> Self {
        let adsr = params.adsr();
        Self {
            params,
            position: 0.0,
            pitch: 0,
            velocity: 0.0,
            pitch_ratio: 0.,
            state: VoiceState::Free,
            env: Envelope::new(adsr),
            sample,
            gate: 0.0,
        }
    }

    fn process(&mut self, buf: &mut [Stereo]) -> ProcessStatus {
        let sample = self.sample.as_ref();
        self.env.update(self.params.adsr());

        for dst_frame in buf.iter_mut() {
            let pos = self.position as usize;
            let weight = self.position - pos as f32;
            let inverse_weight = 1.0 - weight;

            let mut frame = sample[pos] * inverse_weight;
            if pos < sample.len() - 1 {
                frame += sample[pos + 1] * weight;
            }

            *dst_frame += frame * self.velocity * self.env.value(self.gate) as f32;
            self.position += self.pitch_ratio;
            if self.position >= sample.len() as f32 {
                self.state = VoiceState::Free;
                return ProcessStatus::Idle;
            }
        }
        if self.env.state == EnvelopeState::Idle {
            self.state = VoiceState::Free;
            return ProcessStatus::Idle;
        }
        ProcessStatus::Continue
    }

    fn note_off(&mut self) {
        self.gate = 0.0;
    }
}

#[derive(Clone)]
pub struct Sound {
    offset: usize,
    buf: Arc<Buffer>,
    sample_rate: usize,
}

impl Sound {
    fn new(buf: Buffer, offset: usize, sample_rate: usize) -> Self {
        Self {
            buf: Arc::new(buf),
            offset,
            sample_rate,
        }
    }
}

pub fn load_file(path: &Utf8PathBuf) -> Result<Sound> {
    let mut wav = WavReader::open(path.clone())?;
    let wav_spec = wav.spec();
    let bit_depth = wav_spec.bits_per_sample as f32;

    let samples: Vec<f32> = match wav_spec.sample_format {
        SampleFormat::Int => wav
            .samples::<i32>()
            .map(|s| s.unwrap() as f32 / (f32::powf(2., bit_depth - 1.)))
            .collect::<Vec<f32>>(),
        SampleFormat::Float => wav
            .samples::<f32>()
            .map(|s| s.unwrap())
            .collect::<Vec<f32>>(),
    };

    let frames: Vec<Stereo> = samples
        .chunks(wav_spec.channels as usize)
        .map(|f| {
            let left = *f.first().unwrap();
            let right = *f.get(1).unwrap_or(&left);
            Frame::new([left, right])
        })
        .collect();

    const SILENCE: f32 = 0.01;
    let mut offset = 0;
    for (i, frame) in frames.iter().enumerate() {
        if frame.channel(0) < SILENCE && frame.channel(1) < SILENCE {
            continue;
        } else {
            offset = i;
            break;
        }
    }
    Ok(Sound::new(frames, offset, wav_spec.sample_rate as usize))
}

pub struct Sampler {
    voices: Vec<Voice>,
    events: Vec<Event>,
    sound: Sound,
    params: Arc<SamplerParams>,
}

impl Sampler {
    pub fn new(sound: Sound) -> Self {
        let mut voices = Vec::with_capacity(12);
        let params = Arc::new(SamplerParams::default());
        for _ in 0..voices.capacity() {
            voices.push(Voice::new(params.clone(), sound.buf.clone()));
        }
        Self {
            voices,
            events: Vec::with_capacity(64),
            sound,
            params,
        }
    }

    fn note_on(&mut self, track_id: TrackId, pitch: u8, velocity: u8) {
        if let Some(voice) = self.voices.iter_mut().find(|v| v.state == VoiceState::Free) {
            voice.gate = 1.0;
            voice.state = VoiceState::Busy(track_id);
            voice.env = Envelope::new(self.params.adsr());
            voice.pitch = pitch;
            voice.velocity =
                params::db_to_amp(map(velocity.into(), (0.0, 127.0), (-60.0, 0.0))) as f32;

            let pitch = pitch as i8 - ROOT_PITCH as i8;
            voice.pitch_ratio = f32::powf(2., pitch as f32 / 12.0)
                * (self.sound.sample_rate as f32 / SAMPLE_RATE as f32);
            voice.position = self.sound.offset as f32;
        } else {
            eprintln!("dropped event");
        }
    }

    fn send_event(&mut self, ev: &Event) {
        match ev.note {
            Note::On(pitch, velocity) => self.note_on(ev.track_id, pitch, velocity),
            Note::Off => {
                for voice in &mut self.voices.iter_mut() {
                    if let VoiceState::Busy(track_id) = voice.state {
                        if track_id == ev.track_id {
                            voice.note_off();
                        }
                    }
                }
            }
        }
    }

    fn process_block(&mut self, ctx: &mut ProcessContext, range: &Range<usize>) -> ProcessStatus {
        let mut status = ProcessStatus::Idle;
        for voice in &mut self.voices.iter_mut() {
            if let VoiceState::Busy(track_id) = voice.state {
                let buf = ctx.track_buffer(track_id, range);
                let voice_status = voice.process(buf);
                if let ProcessStatus::Continue = voice_status {
                    status = voice_status
                }
            }
        }
        status
    }
}

impl Plugin for Sampler {
    fn process(&mut self, ctx: &mut ProcessContext) -> ProcessStatus {
        let mut last_offset = 0;
        let mut range = 0..ctx.num_frames;
        for i in 0..self.events.len() {
            let ev = self.events[i];
            // Don't call process until we've read all events with the same
            // offset (e.g. a chord)
            if ev.offset != last_offset {
                range.end = ev.offset;
                self.process_block(ctx, &range);
                range.start = range.end;
                range.end = ctx.num_frames;
            }
            last_offset = ev.offset;
            self.send_event(&ev);
        }
        range.end = ctx.num_frames;
        self.events.clear();
        self.process_block(ctx, &range)
    }

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn send_event(&mut self, event: Event) {
        self.events.push(event);
    }
}

fn map(v: f64, from: (f64, f64), to: (f64, f64)) -> f64 {
    (v - from.0) * (to.1 - to.0) / (from.1 - from.0) + to.0
}

pub fn can_load_file(path: &Utf8PathBuf) -> bool {
    path.extension().map_or(false, |ext| ext == "wav")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Note, Track};
    use std::collections::HashMap;

    #[test]
    fn sampler_process() {
        let mut tracks = HashMap::new();
        let track1 = TrackId::new();
        let track2 = TrackId::new();

        tracks.insert(track1, Box::new(Track::default()));
        tracks.insert(track2, Box::new(Track::default()));
        let sample = Stereo::new([0.5, 0.5]);

        let sound = Sound::new(vec![sample; 16], 0, 44100);
        let mut sampler = Sampler::new(sound);
        let note = Note::On(ROOT_PITCH, 127);

        let ev = Event::new(8, track1, note);
        Plugin::send_event(&mut sampler, ev);

        let ev = Event::new(16, track2, note);
        Plugin::send_event(&mut sampler, ev);

        let buf_size = 32;
        let mut ctx = ProcessContext::new(&mut tracks, buf_size);
        sampler.process(&mut ctx);

        let buf = ctx.track_buffer(track1, &(0..buf_size));
        assert_eq!(vec![Stereo::ZERO; 8], buf[0..8]);
        // TODO: check for the actual sample value here, but easier if we can disable
        // envelope.
        assert_ne!(vec![Stereo::ZERO; 16], buf[8..24]);
        assert_eq!(vec![Stereo::ZERO; 8], buf[24..32]);

        let buf = ctx.track_buffer(track2, &(0..buf_size));
        assert_eq!(vec![Stereo::ZERO; 16], buf[0..16]);
        assert_ne!(vec![Stereo::ZERO; 16], buf[16..32]);
    }
}
