use std::collections::HashMap;
use std::sync::Arc;
use std::vec;

use ringbuf::Consumer;
use triple_buffer::Input;

use crate::app::{AppState, AudioContext, DeviceID, EngineState, SharedState, TrackID, TrackType};
use crate::audio::{Buffer, Rms, Stereo};
use crate::params::Params;
use crate::pattern::{NoteEvent, DEFAULT_VELOCITY};
use crate::sampler::{Sampler, Sound, ROOT_PITCH};
use crate::{INTERNAL_BUFFER_SIZE, SAMPLE_RATE};

pub const INSTRUMENT_TRACKS: usize = 16;
pub const TOTAL_TRACKS: usize = INSTRUMENT_TRACKS + 1; // master track
pub const TICKS_PER_LINE: usize = 12;
const MAX_TRACK_EFFECTS: usize = 5;

const RMS_WINDOW_SIZE: usize = SAMPLE_RATE as usize / 10 * 3;

pub enum EngineCommand {
    PreviewSound(Sound),
    CreateTrack(TrackID, Box<Track>),
    CreateDevice(TrackID, DeviceID, Box<dyn Device + Send>),
}

pub trait Device {
    fn render(&mut self, ctx: AudioContext, buffer: &mut [Stereo]);
    fn send_event(&mut self, _ctx: AudioContext, _event: &NoteEvent) {}
    fn params(&self) -> Arc<dyn Params>;
}

pub struct Track {
    devices: HashMap<DeviceID, Box<dyn Device + Send>>,
    buf: Buffer,
    rms: Rms,
}

impl Track {
    pub fn new() -> Self {
        Self {
            rms: Rms::new(RMS_WINDOW_SIZE),
            buf: vec![Stereo::ZERO; INTERNAL_BUFFER_SIZE],
            devices: HashMap::with_capacity(MAX_TRACK_EFFECTS),
        }
    }
}

pub struct Engine {
    state: EngineState,
    state_buf: Input<EngineState>,
    tracks: HashMap<TrackID, Box<Track>>,
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
        let preview = Sampler::new();

        Self {
            tracks,
            state,
            state_buf,
            sum_buf: vec![Stereo::ZERO; INTERNAL_BUFFER_SIZE],
            preview,
            consumer,
            samples_to_tick: 0,
        }
    }

    pub fn render(&mut self, state: &AppState, buffer: &mut [Stereo]) {
        let ctx = AudioContext::new(state);
        self.run_commands(ctx);

        let mut buffer = buffer;
        while let Some(block_size) = self.next_block(ctx, buffer.len()) {
            for track_info in ctx
                .tracks()
                .iter()
                .filter(|t| matches!(t.track_type, TrackType::Instrument))
            {
                let track = self.tracks.get_mut(&track_info.id).unwrap();
                for device in track_info.devices.iter() {
                    let device = track.devices.get_mut(&device.id).unwrap();
                    device.render(ctx, &mut track.buf[..block_size]);
                }
                for j in 0..block_size {
                    let frame = track.buf[j] * track_info.volume.val() as f32;
                    track.rms.add_frame(frame);
                    self.sum_buf[j] += frame;
                    track.buf[j] = Stereo::ZERO;
                }
            }

            let bus_info = ctx.master_bus();
            let bus = self.tracks.get_mut(&bus_info.id).unwrap();

            for (i, output) in buffer.iter_mut().enumerate().take(block_size) {
                let frame = self.sum_buf[i] * bus_info.volume.val() as f32;
                bus.rms.add_frame(frame);
                *output = frame;
                self.sum_buf[i] = Stereo::ZERO;
            }

            self.preview.render(ctx, &mut buffer[..block_size]);
            buffer = &mut buffer[block_size..];
        }

        for track in ctx.tracks().iter() {
            let track_data = self.tracks.get_mut(&track.id).unwrap();
            track.update_rms(amp_to_db(track_data.rms.value()));
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

                for note in pattern.notes(self.state.current_tick) {
                    if ctx.is_track_muted(note.track as usize) {
                        // TODO: trigger fade out for muted channels so sounds with long
                        // release don't keep playing
                        continue;
                    }
                    let track_id = ctx.tracks()[note.track as usize].id;
                    let track = self.tracks.get_mut(&track_id).unwrap();
                    for device in &mut track.devices.values_mut() {
                        device.send_event(ctx, &note);
                    }
                }

                self.state.current_tick += 1;
                if self.state.current_tick >= pattern.ticks() {
                    self.state.current_tick = 0;
                    curr_pattern = ctx.next_pattern(curr_pattern);
                }
                self.state.current_pattern = curr_pattern;
            }

            let samples_to_tick = (SAMPLE_RATE * 60.)
                / (TICKS_PER_LINE as u16 * ctx.lines_per_beat() * ctx.bpm()) as f64;
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
