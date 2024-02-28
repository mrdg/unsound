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
use crate::params::Params;
use crate::pattern::{Note, DEFAULT_VELOCITY};
use crate::{INTERNAL_BUFFER_SIZE, SAMPLE_RATE};

pub const INSTRUMENT_TRACKS: usize = 16;
pub const TOTAL_TRACKS: usize = INSTRUMENT_TRACKS + 1; // master track
pub const TICKS_PER_LINE: usize = 12;

const RMS_WINDOW_SIZE: usize = SAMPLE_RATE as usize / 10 * 3;

pub enum EngineCommand {
    CreateTrack(TrackId, Box<Track>),
    CreateInstrument(DeviceId, Box<dyn Plugin + Send>),
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
        let instruments = HashMap::with_capacity(TOTAL_TRACKS);
        tracks.insert(preview_track_id, Box::new(Track::default()));

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
        let mut curr_pattern = self.state.current_pattern;
        let pattern = state.pattern(curr_pattern).unwrap_or_else(|| {
            // The active pattern can be deleted while we're playing it. Continue with the
            // next one if that happens, which should always be safe to unwrap.
            curr_pattern = state.next_pattern(curr_pattern);
            state.pattern(curr_pattern).unwrap()
        });

        for event in pattern.events(self.state.current_tick) {
            if let Some(instr) = &state.instruments[event.instrument] {
                let track_id = state.tracks[event.track].id;
                let track = self.tracks.get_mut(&track_id).unwrap();

                if let Some((tick, instr_id)) = track.last_event {
                    if tick != self.total_ticks {
                        let instr = self.instruments.get_mut(&instr_id).unwrap();
                        instr.send_event(Event::new(offset, track_id, Note::Off));
                    }
                }

                track.last_event = Some((self.total_ticks, instr.id));
                if let Note::Off = event.note {
                    track.last_event = None;
                }

                let instr = self.instruments.get_mut(&instr.id).unwrap();
                instr.send_event(Event::new(offset, track_id, event.note));
            }
        }

        self.state.current_tick += 1;
        if self.state.current_tick >= pattern.ticks() {
            self.state.current_tick = 0;
            curr_pattern = state.next_pattern(curr_pattern);
        }
        self.state.current_pattern = curr_pattern;
    }

    pub fn process(&mut self, state: &AppState, buffer: &mut [Stereo]) {
        self.update_state(state);
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

    fn update_state(&mut self, state: &AppState) {
        self.master.mute.target = if state.is_playing { 1.0 } else { 0.0 };
        for track_settings in &state.tracks {
            if let Some(track) = self.tracks.get_mut(&track_settings.id) {
                let track_muted = track_settings.muted || !state.is_playing;
                track.mute.target = if track_muted { 0.0 } else { 1.0 };
                track.volume.target = track_settings.volume.val();
            }
        }
    }
}

pub struct Track {
    pub buf: Buffer,
    pub rms_out: Arc<[AtomicF64; 2]>,
    rms: Rms,
    volume: SmoothedValue,
    mute: SmoothedValue,
    /// Time and instrument id for the last note on event played on this track. This allows sending
    /// a note off to that device when a new event is played on this track.
    last_event: Option<(u64, DeviceId)>,
}

impl Track {
    pub fn new(volume: f64) -> Self {
        Self {
            rms: Rms::new(RMS_WINDOW_SIZE),
            rms_out: Arc::new([AtomicF64::new(0.0), AtomicF64::new(0.0)]),
            volume: SmoothedValue::new(volume),
            buf: vec![Stereo::ZERO; INTERNAL_BUFFER_SIZE],
            mute: SmoothedValue::new(1.0),
            last_event: None,
        }
    }

    fn process(&mut self, buf: &mut [Stereo]) {
        for (i, out) in buf.iter_mut().enumerate() {
            let frame = self.buf[i] * self.volume.value() as f32 * self.mute.value() as f32;
            self.rms.add_frame(frame);
            *out += frame;
            self.buf[i] = Stereo::ZERO;
        }
        let v = self.rms.value().to_db();
        self.rms_out[0].store(v.channel(0) as f64, Ordering::Relaxed);
        self.rms_out[1].store(v.channel(1) as f64, Ordering::Relaxed);
    }
}

impl Default for Track {
    fn default() -> Self {
        Self::new(1.0)
    }
}

struct Device {
    inner: Box<dyn Plugin + Send>,
    status: Option<ProcessStatus>,
    deleted: bool,
}

impl Device {
    fn new(inner: Box<dyn Plugin + Send>) -> Self {
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

pub struct SmoothedValue {
    current: f64,
    target: f64,
}

impl SmoothedValue {
    fn new(value: f64) -> Self {
        Self {
            current: value,
            target: value,
        }
    }

    fn value(&mut self) -> f64 {
        self.current = 0.01 * self.target + 0.99 * self.current;
        self.current
    }
}
