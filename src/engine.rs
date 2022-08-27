use std::collections::HashMap;
use std::vec;

use ringbuf::Consumer;
use triple_buffer::Input;

use crate::app::{AppState, AudioContext, DeviceID, EngineState, SharedState, TrackID};
use crate::audio::{Buffer, Rms, Stereo};
use crate::params::Params;
use crate::pattern::{NoteEvent, DEFAULT_VELOCITY};
use crate::sampler::{Sampler, Sound, ROOT_PITCH};
use crate::{INTERNAL_BUFFER_SIZE, SAMPLE_RATE};

pub const INSTRUMENT_TRACKS: usize = 16;
pub const TOTAL_TRACKS: usize = INSTRUMENT_TRACKS + 1; // master track
const MAX_TRACK_EFFECTS: usize = 5;

const RMS_WINDOW_SIZE: usize = SAMPLE_RATE as usize / 10 * 3;

pub enum EngineCommand {
    PreviewSound(Sound),
    CreateTrack(TrackID, Box<Track>),
    CreateDevice(TrackID, DeviceID, Box<dyn Device + Send>),
}

pub trait Device {
    fn render(&mut self, ctx: DeviceContext, buffer: &mut [Stereo]);
    fn send_event(&mut self, _ctx: DeviceContext, _event: &NoteEvent) {}
}

#[derive(Copy, Clone)]
pub struct DeviceContext<'a> {
    sounds: &'a Vec<Option<Sound>>,
    params: &'a Params,
}

impl<'a> DeviceContext<'a> {
    fn new(track_idx: usize, device_idx: usize, ctx: &'a AudioContext) -> Self {
        let device = ctx.device(track_idx, device_idx);
        Self {
            sounds: ctx.sounds(),
            params: &device.params,
        }
    }

    fn for_preview(ctx: &'a AudioContext, params: &'a Params) -> Self {
        Self {
            sounds: ctx.sounds(),
            params,
        }
    }

    pub fn sound(&self, idx: usize) -> &Option<Sound> {
        self.sounds.get(idx).unwrap_or(&None)
    }

    pub fn params(&self) -> &Params {
        self.params
    }
}

pub struct Track {
    devices: HashMap<DeviceID, Box<dyn Device + Send>>,
    rms: Rms,
    rms_buf: Input<Stereo>,
}

impl Track {
    pub fn new(rms_buf: Input<Stereo>) -> Self {
        Self {
            rms: Rms::new(RMS_WINDOW_SIZE),
            devices: HashMap::with_capacity(MAX_TRACK_EFFECTS),
            rms_buf,
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
    preview_params: Params,
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

        let params = Sampler::params();
        let preview = Sampler::new(&params);

        Self {
            tracks,
            state,
            state_buf,
            track_buf: vec![Stereo::ZERO; INTERNAL_BUFFER_SIZE],
            sum_buf: vec![Stereo::ZERO; INTERNAL_BUFFER_SIZE],
            preview,
            preview_params: params,
            consumer,
            samples_to_tick: 0,
        }
    }

    pub fn render(&mut self, state: &AppState, buffer: &mut [Stereo]) {
        let ctx = AudioContext::new(state);
        self.run_commands(ctx);

        let mut buffer = buffer;
        while let Some(block_size) = self.next_block(ctx, buffer.len()) {
            for (i, track_info) in ctx.instrument_tracks().enumerate() {
                let track = self.tracks.get_mut(&track_info.id).unwrap();
                for (j, device) in track_info.devices.iter().enumerate() {
                    let device_ctx = DeviceContext::new(i, j, &ctx);
                    let device = track.devices.get_mut(&device.id).unwrap();
                    device.render(device_ctx, &mut self.track_buf[..block_size]);
                }

                // TODO: measure rms after volume fader
                track.rms.add_frames(&self.track_buf[..block_size]);

                for j in 0..block_size {
                    self.sum_buf[j] += self.track_buf[j] * ctx.track_volume(i) as f32;
                    self.track_buf[j] = Stereo::ZERO;
                }
            }

            let master = self.tracks.get_mut(&ctx.master_track().id).unwrap();
            master.rms.add_frames(&self.sum_buf[..block_size]);

            for (i, frame) in buffer.iter_mut().enumerate().take(block_size) {
                *frame = self.sum_buf[i];
                self.sum_buf[i] = Stereo::ZERO;
            }

            let ctx = DeviceContext::for_preview(&ctx, &self.preview_params);
            self.preview.render(ctx, &mut buffer[..block_size]);
            buffer = &mut buffer[block_size..];
        }

        for track in ctx.tracks().iter() {
            let track_data = self.tracks.get_mut(&track.id).unwrap();
            track_data.rms_buf.write(amp_to_db(track_data.rms.value()));
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

                for note in pattern.notes(self.state.current_tick as u64) {
                    if ctx.is_track_muted(note.track as usize) {
                        // TODO: trigger fade out for muted channels so sounds with long
                        // release don't keep playing
                        continue;
                    }
                    let track_id = ctx.tracks()[note.track as usize].id;
                    let track = self.tracks.get_mut(&track_id).unwrap();
                    for (i, device) in &mut track.devices.values_mut().enumerate() {
                        let ctx = DeviceContext::new(note.track as usize, i, &ctx);
                        device.send_event(ctx, &note);
                    }
                }

                self.state.current_tick += 1;
                if self.state.current_tick >= pattern.len() {
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

    fn run_commands(&mut self, ctx: AudioContext) {
        while let Some(cmd) = self.consumer.pop() {
            match cmd {
                EngineCommand::PreviewSound(snd) => {
                    let ctx = DeviceContext::for_preview(&ctx, &self.preview_params);
                    self.preview
                        .note_on(&snd, ctx, ROOT_PITCH, DEFAULT_VELOCITY);
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
