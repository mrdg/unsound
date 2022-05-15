use std::collections::HashMap;
use std::sync::Arc;
use std::vec;

use ringbuf::Consumer;
use triple_buffer::Input;

use crate::app::{AppState, AudioContext, DeviceID, EngineState, SharedState, TrackID};
use crate::audio::{Buffer, Rms, Stereo};
use crate::pattern::NoteEvent;
use crate::sampler::{Sampler, Sound, ROOT_PITCH};
use crate::{INTERNAL_BUFFER_SIZE, SAMPLE_RATE};

pub const INSTRUMENT_TRACKS: usize = 16;
pub const TOTAL_TRACKS: usize = INSTRUMENT_TRACKS + 1; // master track
const MAX_TRACK_EFFECTS: usize = 5;

const RMS_WINDOW_SIZE: usize = SAMPLE_RATE as usize / 10 * 3;

pub enum EngineCommand {
    PreviewSound(Arc<Sound>),
    CreateTrack(TrackID, Box<Track>),
    CreateDevice(TrackID, DeviceID, Box<dyn Device + Send>),
}

pub trait Device {
    fn render(&mut self, ctx: TrackContext, buffer: &mut [Stereo]);
    fn send_event(&mut self, _ctx: TrackContext, _event: &NoteEvent) {}
}

pub struct Volume {}

impl Device for Volume {
    fn render(&mut self, ctx: TrackContext, buffer: &mut [Stereo]) {
        for frame in buffer.iter_mut() {
            *frame = *frame * ctx.volume;
        }
    }
}

#[derive(Copy, Clone)]
pub struct TrackContext<'a> {
    volume: f32,
    sounds: &'a Vec<Option<Arc<Sound>>>,
}

impl<'a> TrackContext<'a> {
    fn new(track: usize, ctx: &'a AudioContext) -> Self {
        Self {
            volume: ctx.track_volume(track) as f32,
            sounds: ctx.sounds(),
        }
    }

    fn for_preview(ctx: &'a AudioContext) -> Self {
        Self {
            volume: 0.6,
            sounds: ctx.sounds(),
        }
    }

    pub fn sound(&self, idx: usize) -> &Option<Arc<Sound>> {
        &self.sounds[idx]
    }
}

pub struct Track {
    devices: HashMap<DeviceID, Box<dyn Device + Send>>,
    rms: Rms,
}

impl Track {
    pub fn new() -> Self {
        Self {
            rms: Rms::new(RMS_WINDOW_SIZE),
            devices: HashMap::with_capacity(MAX_TRACK_EFFECTS),
        }
    }
}

pub struct Engine {
    state: EngineState,
    state_buf: Input<EngineState>,
    tracks: HashMap<TrackID, Box<Track>>,
    track_buf: Buffer,
    sum_buf: Buffer,
    preview: Sampler,
    consumer: Consumer<EngineCommand>,
    samples_to_tick: usize,
}

impl Engine {
    pub fn new(
        state: EngineState,
        state_buf: Input<EngineState>,
        consumer: Consumer<EngineCommand>,
    ) -> Engine {
        let tracks = HashMap::with_capacity(TOTAL_TRACKS);

        Self {
            tracks,
            state,
            state_buf,
            track_buf: vec![Stereo::ZERO; INTERNAL_BUFFER_SIZE],
            sum_buf: vec![Stereo::ZERO; INTERNAL_BUFFER_SIZE],
            preview: Sampler::new(),
            consumer,
            samples_to_tick: 0,
        }
    }

    pub fn render(&mut self, state: &AppState, buffer: &mut [Stereo]) {
        self.run_commands();
        let ctx = AudioContext::new(state);

        let mut buffer = buffer;
        while let Some(block_size) = self.next_block(ctx, buffer.len()) {
            for (i, track_info) in ctx.instrument_tracks().enumerate() {
                let track = self.tracks.get_mut(&track_info.id).unwrap();

                let ctx = TrackContext::new(i, &ctx);
                for device_id in &track_info.devices {
                    let effect = track.devices.get_mut(device_id).unwrap();
                    effect.render(ctx, &mut self.track_buf[..block_size]);
                }

                track.rms.add_frames(&self.track_buf[..block_size]);

                for j in 0..block_size {
                    self.sum_buf[j] += self.track_buf[j];
                    self.track_buf[j] = Stereo::ZERO;
                }
            }

            let master = self.tracks.get_mut(&ctx.master_track().id).unwrap();
            master.rms.add_frames(&self.sum_buf[..block_size]);

            for (i, frame) in buffer.iter_mut().enumerate().take(block_size) {
                *frame = self.sum_buf[i];
                self.sum_buf[i] = Stereo::ZERO;
            }

            let ctx = TrackContext::for_preview(&ctx);
            self.preview.render(ctx, buffer);
            buffer = &mut buffer[block_size..];
        }

        for (i, track) in ctx.tracks().iter().enumerate() {
            let track_data = self.tracks.get(&track.id).unwrap();
            self.state.rms[i] = amp_to_db(track_data.rms.value());
        }

        let buf = self.state_buf.input_buffer();
        buf.clone_from(&self.state);
        self.state_buf.publish();
    }

    fn next_block(&mut self, ctx: AudioContext<'_>, frames: usize) -> Option<usize> {
        if self.samples_to_tick == 0 {
            if ctx.is_playing() {
                let mut curr_pattern = self.state.current_pattern;

                let pattern = ctx.pattern(curr_pattern).unwrap_or_else(|| {
                    // The active pattern can be deleted while we're playing it. Continue with the
                    // next one if that happens, which should always be safe to unwrap.
                    curr_pattern = ctx.next_pattern(curr_pattern);
                    ctx.pattern(curr_pattern).unwrap()
                });

                for note in pattern.iter_notes(self.state.current_tick as u64) {
                    if ctx.is_track_muted(note.track as usize) {
                        // TODO: trigger fade out for muted channels so sounds with long
                        // release don't keep playing
                        continue;
                    }
                    let track_id = ctx.tracks()[note.track as usize].id;
                    let track = self.tracks.get_mut(&track_id).unwrap();
                    for device in &mut track.devices.values_mut() {
                        let ctx = TrackContext::new(note.track as usize, &ctx);
                        device.send_event(ctx, &note);
                    }
                }

                self.state.current_tick += 1;
                if self.state.current_tick >= pattern.length {
                    self.state.current_tick = 0;
                    curr_pattern = ctx.next_pattern(curr_pattern);
                }
                self.state.current_pattern = curr_pattern;
            }

            let samples_to_tick = (SAMPLE_RATE * 60.) / (ctx.lines_per_beat() * ctx.bpm()) as f64;
            self.samples_to_tick = samples_to_tick.round() as usize;
        }

        let block_size = usize::min(frames, self.samples_to_tick);
        self.samples_to_tick -= block_size;
        if block_size > 0 {
            Some(block_size)
        } else {
            None
        }
    }

    fn run_commands(&mut self) {
        while let Some(cmd) = self.consumer.pop() {
            match cmd {
                EngineCommand::PreviewSound(snd) => {
                    self.preview.note_on(snd, 0, ROOT_PITCH, 80);
                }
                EngineCommand::CreateTrack(track_id, track) => {
                    self.tracks.insert(track_id, track);
                }
                EngineCommand::CreateDevice(track_id, device_id, effect) => {
                    let track = self.tracks.get_mut(&track_id).unwrap();
                    track.devices.insert(device_id, effect);
                }
            }
        }
    }
}

fn amp_to_db(frame: Stereo) -> Stereo {
    frame.map(|sample| 20.0 * f32::log10(sample.abs()))
}
