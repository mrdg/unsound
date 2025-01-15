use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::vec;

use atomic_float::AtomicF64;
use ringbuf::Consumer;
use triple_buffer::Input;

use crate::app::{AppState, DeviceId, EngineState, TrackId};
use crate::audio::{Buffer, Rms, Stereo};
use crate::params::{self, Param, ParamInfo, Params};
use crate::pattern::DEFAULT_VELOCITY;
use crate::{INTERNAL_BUFFER_SIZE, SAMPLE_RATE};
use param_derive::Params;

pub const INSTRUMENT_TRACKS: usize = 16;
pub const PREVIEW_INSTRUMENTS_CACHE_SIZE: usize = 10;
pub const MAX_INSTRUMENTS: usize = INSTRUMENT_TRACKS + PREVIEW_INSTRUMENTS_CACHE_SIZE;
pub const TOTAL_TRACKS: usize = INSTRUMENT_TRACKS + 1; // add 1 for master track
pub const TICKS_PER_LINE: usize = 12;

const RMS_WINDOW_SIZE: usize = SAMPLE_RATE as usize / 10 * 3;

pub enum EngineCommand {
    CreateTrack(TrackId, Box<Track>),
    CreateInstrument(DeviceId, basedrop::Owned<Box<dyn Plugin + Send>>),
    DeleteInstrument(DeviceId),
    PlayNote(DeviceId, TrackId, u8),
}

pub struct Engine {
    state: EngineState,
    state_buf: Input<EngineState>,
    instruments: HashMap<DeviceId, Device>,
    tracks: HashMap<TrackId, Box<Track>>,
    master: Track,
    preview_track_id: TrackId,
    consumer: Consumer<EngineCommand>,
    samples_to_tick: usize,
    total_ticks: u64,
}

impl Engine {
    pub fn new(
        state: EngineState,
        state_buf: Input<EngineState>,
        consumer: Consumer<EngineCommand>,
        master: Track,
        preview_track_id: TrackId,
    ) -> Engine {
        let mut tracks = HashMap::with_capacity(TOTAL_TRACKS);
        tracks.insert(preview_track_id, Box::new(Track::new()));

        // Double the capacity here. Deleting instruments is asynchronous
        // so we might have a few more in flight than the max
        let instruments = HashMap::with_capacity(2 * MAX_INSTRUMENTS);

        Self {
            instruments,
            tracks,
            state,
            state_buf,
            master,
            preview_track_id,
            consumer,
            samples_to_tick: 0,
            total_ticks: 0,
        }
    }

    fn tick(&mut self, state: &AppState, num_frames: usize) {
        let mut num_frames = num_frames;
        let mut offset = 0;
        while num_frames > 0 {
            if self.samples_to_tick == 0 {
                self.dispatch_events(state, offset);
                let samples_to_tick = (SAMPLE_RATE * 60.)
                    / (TICKS_PER_LINE as u16 * state.lines_per_beat * state.bpm) as f64;
                self.samples_to_tick = samples_to_tick.round() as usize;
                self.total_ticks += 1;
            }
            offset = usize::min(num_frames, self.samples_to_tick);
            self.samples_to_tick -= offset;
            num_frames -= offset;
        }
    }

    fn dispatch_events(&mut self, state: &AppState, offset: usize) {
        if !state.is_playing {
            return;
        }
        let mut seq_idx = self.state.current_sequence;
        let sequence = state.sequence(seq_idx).unwrap_or_else(|| {
            // The active sequence can be deleted while we're playing it. Continue with the
            // next one if that happens, which should always be safe to unwrap.
            seq_idx = state.next_sequence(seq_idx);
            state.sequence(seq_idx).unwrap()
        });

        // TODO: do a binary search?
        for instruction in &sequence.instructions {
            if instruction.offset > self.state.current_tick {
                break;
            }
            if instruction.offset == self.state.current_tick {
                if let Some(instr) = &state.instruments[instruction.instrument] {
                    let track_id = &state.tracks[instruction.track].id;
                    let track = self.tracks.get_mut(track_id).unwrap();

                    if let Some((tick, instr_id)) = track.last_event {
                        if tick != self.total_ticks {
                            let instr = self.instruments.get_mut(&instr_id).unwrap();
                            instr.send_event(Event::new(offset, *track_id, Note::Off));
                        }
                    }

                    track.last_event = Some((self.total_ticks, instr.id));
                    if let Note::Off = instruction.note {
                        track.last_event = None;
                    }

                    let instr = self.instruments.get_mut(&instr.id).unwrap();
                    instr.send_event(Event::new(offset, *track_id, instruction.note));
                }
            }
        }

        self.state.current_tick += 1;
        if self.state.current_tick >= sequence.length {
            self.state.current_tick = 0;
            seq_idx = state.next_sequence(seq_idx);
        }
        self.state.current_sequence = seq_idx;
    }

    pub fn process(&mut self, state: &AppState, buffer: &mut [Stereo]) {
        self.run_commands(state);
        self.tick(state, buffer.len());

        for instr in &mut self.instruments.values_mut() {
            let mut ctx = ProcessContext::new(&mut self.tracks, buffer.len());
            instr.process(&mut ctx);
        }

        for (id, track) in self.tracks.iter_mut() {
            if *id == self.preview_track_id {
                // Preview track processes directly into the output buffer
                continue;
            }
            track.process(&mut self.master.buf[..buffer.len()]);
        }
        self.master.process(buffer);

        let preview = self.tracks.get_mut(&self.preview_track_id).unwrap();
        preview.process(buffer);

        self.state_buf.input_buffer().clone_from(&self.state);
        self.state_buf.publish();
    }

    fn run_commands(&mut self, _state: &AppState) {
        while let Some(cmd) = self.consumer.pop() {
            match cmd {
                EngineCommand::CreateTrack(track_id, track) => {
                    self.tracks.insert(track_id, track);
                }
                EngineCommand::CreateInstrument(device_id, instrument) => {
                    self.instruments.insert(device_id, Device::new(instrument));
                }
                EngineCommand::DeleteInstrument(device_id) => {
                    self.instruments.retain(|_, d| !d.deleted || !d.is_idle());
                    let instr = self.instruments.get_mut(&device_id).unwrap();
                    for (track_id, track) in &mut self.tracks {
                        if let Some((_, id)) = track.last_event {
                            if id == device_id {
                                track.last_event = None;
                                instr.send_event(Event::new(0, *track_id, Note::Off))
                            }
                        }
                    }
                    instr.delete();
                }
                EngineCommand::PlayNote(device_id, track_id, pitch) => {
                    let track = self.tracks.get_mut(&track_id).unwrap();
                    if let Some((_, instr_id)) = track.last_event {
                        if track_id == self.preview_track_id {
                            let instr = self.instruments.get_mut(&instr_id).unwrap();
                            instr.send_event(Event::new(0, track_id, Note::Off));
                        }
                    }
                    let instr = self.instruments.get_mut(&device_id).unwrap();
                    let note = Note::On(pitch, DEFAULT_VELOCITY);
                    track.last_event = Some((0, device_id));
                    instr.send_event(Event::new(0, track_id, note));
                }
            }
        }
    }
}

impl Default for Track {
    fn default() -> Self {
        Track::new()
    }
}

pub struct Track {
    pub buf: Buffer,
    pub rms_out: Arc<[AtomicF64; 2]>,
    rms: Rms,
    /// Time and instrument id for the last note on event played on this track. This allows sending
    /// a note off to that device when a new event is played on this track.
    last_event: Option<(u64, DeviceId)>,

    params: Arc<TrackParams>,
}

#[derive(Params)]
pub struct TrackParams {
    volume: Param,
    mute: Param,
}

impl TrackParams {
    fn new() -> Self {
        Self {
            volume: Param::new(
                -6.0,
                ParamInfo::new("Volume", -60, 3)
                    .with_steps([0.25, 1.0])
                    .with_smoothing(params::Smoothing::exp_default())
                    .with_map(params::db_to_amp),
            ),
            mute: Param::new(
                1.0,
                ParamInfo::bool("Mute", 0.0).with_smoothing(params::Smoothing::exp_default()),
            ),
        }
    }
}

impl Track {
    pub fn new() -> Self {
        Self {
            rms: Rms::new(RMS_WINDOW_SIZE),
            rms_out: Arc::new([AtomicF64::new(0.0), AtomicF64::new(0.0)]),
            buf: vec![Stereo::ZERO; INTERNAL_BUFFER_SIZE],
            last_event: None,
            params: Arc::new(TrackParams::new()),
        }
    }

    pub fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn process(&mut self, buf: &mut [Stereo]) {
        for (i, out) in buf.iter_mut().enumerate() {
            let frame =
                self.buf[i] * self.params.volume.value() as f32 * self.params.mute.value() as f32;
            self.rms.add_frame(frame);
            *out += frame;
            self.buf[i] = Stereo::ZERO;
        }
        let v = self.rms.value().to_db();
        self.rms_out[0].store(v.channel(0) as f64, Ordering::Relaxed);
        self.rms_out[1].store(v.channel(1) as f64, Ordering::Relaxed);
    }
}

struct Device {
    inner: basedrop::Owned<Box<dyn Plugin + Send>>,
    status: Option<ProcessStatus>,
    deleted: bool,
}

impl Device {
    fn new(inner: basedrop::Owned<Box<dyn Plugin + Send>>) -> Self {
        Self {
            status: None,
            deleted: false,
            inner,
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext) {
        if self.is_idle() {
            return;
        }
        let status = self.inner.process(ctx);
        self.status = Some(status);
    }

    fn send_event(&mut self, ev: Event) {
        if self.deleted {
            return;
        }
        self.inner.send_event(ev);
        self.status = None;
    }

    fn delete(&mut self) {
        self.deleted = true;
    }

    fn is_idle(&self) -> bool {
        matches!(self.status, Some(ProcessStatus::Idle))
    }
}

pub enum ProcessStatus {
    Continue,
    Idle,
}

pub trait Plugin {
    fn process(&mut self, ctx: &mut ProcessContext) -> ProcessStatus;
    fn params(&self) -> Arc<dyn Params>;
    fn send_event(&mut self, event: Event);
}

/// Data passed to a device for processing a single audio buffer
pub struct ProcessContext<'a> {
    pub num_frames: usize,
    pub tracks: &'a mut HashMap<TrackId, Box<Track>>,
}

impl<'a> ProcessContext<'a> {
    pub fn new(tracks: &'a mut HashMap<TrackId, Box<Track>>, num_frames: usize) -> Self {
        Self { num_frames, tracks }
    }

    pub fn track_buffer(&mut self, track_id: TrackId, range: &Range<usize>) -> &mut [Stereo] {
        let track = self.tracks.get_mut(&track_id).unwrap();
        &mut track.buf[range.clone()]
    }
}

#[derive(Clone, Copy)]
pub struct Event {
    /// offset of the event within the audio buffer
    pub offset: usize,
    pub track_id: TrackId,
    pub note: Note,
}

impl Event {
    pub fn new(offset: usize, track_id: TrackId, note: Note) -> Self {
        Self {
            offset,
            track_id,
            note,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Sequence {
    /// length of this sequence in ticks
    pub length: usize,
    pub instructions: Vec<Instruction>,
}

#[derive(Clone, Debug)]
pub struct Instruction {
    pub note: Note,
    /// offset in ticks relative to the start of the sequence
    pub offset: usize,
    pub instrument: usize,
    pub track: usize,
}

impl Instruction {
    pub fn new(note: Note, offset: usize, track: usize, instrument: usize) -> Self {
        Self {
            note,
            offset,
            instrument,
            track,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    On(u8, u8),
    Off,
}
