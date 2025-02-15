use std::iter;
use std::ops::Range;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use atomic_float::AtomicF64;
use get_many_mut::GetManyMutExt;
use ringbuf::{Consumer, Producer};
use triple_buffer::Input;

use crate::app::{AppCommand, AppState, EngineState};
use crate::audio::{self, Buffer, Rms, Stereo};
use crate::params::{self, Param, ParamInfo, Params};
use crate::sampler::{Sampler, Sound};
use crate::SAMPLE_RATE;
use param_derive::Params;

pub const MAX_INSTRUMENTS: usize = 16;
pub const TICKS_PER_LINE: usize = 12;
pub const MAX_TRACKS: usize = MAX_INSTRUMENTS + 1; // add 1 for master
pub const MAX_NODES: usize = MAX_TRACKS + MAX_INSTRUMENTS;
pub const MAX_BUFFERS: usize = MAX_TRACKS + 2; // add 1 for main output and 1 for scratch space
pub const MAIN_OUTPUT: usize = MAX_BUFFERS - 1;
pub const SCRATCH_BUFFER: usize = MAX_BUFFERS - 2;
pub const MASTER_TRACK: usize = 0;
const RMS_WINDOW_SIZE: usize = SAMPLE_RATE as usize / 10 * 3;
const SUBFRAMES_PER_SEC: usize = 282240000; // LCM of common sample rates

pub enum EngineCommand {
    CreateNode(usize, Box<dyn Plugin + Send>),
    DeleteNode(usize),
    PreviewSound(Arc<Sound>),
}

pub struct Engine {
    state: EngineState,
    state_buf: Input<EngineState>,

    nodes: Vec<Node>,
    buffers: Vec<Buffer>,

    /// Time and node index for the last note-on event played for each track. This allows sending
    /// a note off to a node when a new event is played a track.
    last_events: Vec<Option<(u64, usize)>>,

    consumer: Consumer<EngineCommand>,
    producer: Producer<AppCommand>,

    /// Number of subframes until the next tick
    subframe_countdown: usize,
    total_ticks: u64,

    preview: Sampler,
}

impl Engine {
    pub fn new(
        state: EngineState,
        state_buf: Input<EngineState>,
        consumer: Consumer<EngineCommand>,
        producer: Producer<AppCommand>,
    ) -> Engine {
        let mut nodes = Vec::with_capacity(MAX_NODES);
        for _ in 0..MAX_NODES {
            nodes.push(Node::new());
        }
        let mut buffers = Vec::with_capacity(MAX_BUFFERS);
        for _ in 0..MAX_BUFFERS {
            buffers.push(audio::buffer());
        }

        let preview = Sampler::new(Sound::silence());
        let last_events = vec![None; MAX_TRACKS];

        Self {
            nodes,
            state,
            state_buf,
            consumer,
            producer,
            subframe_countdown: 0,
            total_ticks: 0,
            preview,
            buffers,
            last_events,
        }
    }

    fn tick(&mut self, state: &AppState, frames: usize) {
        let subframes_per_sample = SUBFRAMES_PER_SEC / SAMPLE_RATE as usize;
        let mut subframes = frames * subframes_per_sample;
        let mut offset = 0;
        while subframes > 0 {
            if self.subframe_countdown == 0 {
                self.dispatch_events(state, offset / subframes_per_sample);
                let subframes_per_tick = (SUBFRAMES_PER_SEC * 60)
                    / (TICKS_PER_LINE as u16 * state.lines_per_beat * state.bpm) as usize;

                self.subframe_countdown = subframes_per_tick;
                self.total_ticks += 1;
            }
            offset = usize::min(subframes, self.subframe_countdown);
            self.subframe_countdown -= offset;
            subframes -= offset;
        }
    }

    pub fn process(&mut self, state: &AppState, buffer: &mut [Stereo]) {
        let frames = buffer.len();
        self.run_commands(state);
        self.tick(state, frames);

        for entry in &state.node_order {
            let node = &mut self.nodes[entry.node_index];
            if node.is_idle() {
                continue;
            }
            let Some(plugin) = &mut node.inner else {
                continue;
            };
            let mut ctx = ProcessContext::new(&mut self.buffers, frames);
            ctx.mix = Some(&node.mix);
            ctx.buffer_indices = entry.buffers;
            node.status = Some(plugin.process(&mut ctx));
        }
        let mut ctx = ProcessContext::new(&mut self.buffers, frames);
        self.preview.process(&mut ctx);

        let main = &mut self.buffers[MAIN_OUTPUT][..frames];
        for (i, frame) in main.iter_mut().enumerate() {
            buffer[i] = *frame;
            *frame = Stereo::ZERO;
        }

        for buf in self.buffers.iter_mut() {
            for frame in buf {
                *frame = Stereo::ZERO;
            }
        }

        self.state_buf.input_buffer().clone_from(&self.state);
        self.state_buf.publish();
    }

    fn dispatch_events(&mut self, state: &AppState, offset: usize) {
        if !state.is_playing {
            return;
        }
        let mut pattern_idx = self.state.current_pattern;
        let pattern = state.pattern(pattern_idx).unwrap_or_else(|| {
            // The active pattern can be deleted while we're playing it. Continue with the
            // next one if that happens, which should always be safe to unwrap.
            pattern_idx = state.next_pattern(pattern_idx);
            state.pattern(pattern_idx).unwrap()
        });

        for event in &pattern.events {
            if event.offset > self.state.current_tick {
                break;
            }
            if event.offset == self.state.current_tick {
                let node_idx = event.node_index;
                let track_idx = event.track_index;

                if let Some((tick, node_idx)) = self.last_events[track_idx] {
                    if tick != self.total_ticks {
                        let node = &mut self.nodes[node_idx];
                        node.send_event(PluginEvent::new(offset, track_idx, Note::Off));
                    }
                }

                self.last_events[track_idx] = Some((self.total_ticks, node_idx));
                if let Note::Off = event.note {
                    self.last_events[track_idx] = None;
                }

                let node = &mut self.nodes[node_idx];
                node.send_event(PluginEvent::new(offset, track_idx, event.note));
            }
        }

        self.state.current_tick += 1;
        if self.state.current_tick >= pattern.length {
            self.state.current_tick = 0;
            pattern_idx = state.next_pattern(pattern_idx);
        }
        self.state.current_pattern = pattern_idx;
    }

    fn run_commands(&mut self, _state: &AppState) {
        while let Some(cmd) = self.consumer.pop() {
            match cmd {
                EngineCommand::CreateNode(node_idx, plugin) => {
                    let node = &mut self.nodes[node_idx];
                    assert!(node.inner.is_none());
                    node.inner = Some(plugin);
                }
                EngineCommand::DeleteNode(node_idx) => {
                    for (i, node) in self.nodes.iter_mut().enumerate() {
                        if node.inner.is_some() && node.deleted && node.is_quiet() {
                            let plugin = node.reset();
                            if self
                                .producer
                                .push(AppCommand::DropPlugin(i, plugin))
                                .is_err()
                            {
                                eprintln!("failed to return node to app thread");
                            }
                        }
                    }
                    let node = &mut self.nodes[node_idx];
                    for (track_idx, event) in self.last_events.iter_mut().enumerate() {
                        if let Some((_, idx)) = event {
                            if *idx == node_idx {
                                *event = None;
                                node.send_event(PluginEvent::new(0, track_idx, Note::Off))
                            }
                        }
                    }
                    node.delete();
                }
                EngineCommand::PreviewSound(sound) => {
                    let velocity = 80; // TODO: handle this with gain instead?
                    self.preview
                        .send_event(PluginEvent::new(0, MAIN_OUTPUT, Note::Off));
                    self.preview.load(sound);
                    self.preview.send_event(PluginEvent::new(
                        0,
                        MAIN_OUTPUT,
                        Note::On(48, velocity),
                    ));
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
    pub rms_out: Arc<[AtomicF64; 2]>,
    rms: Rms,
    params: Arc<TrackParams>,
}

#[derive(Params)]
pub struct TrackParams {
    volume: Param,
    mute: Param,
    mix: Param,
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
            mix: Param::new(
                1.0,
                ParamInfo::bool("Mix", 1.0).with_smoothing(params::Smoothing::exp_default()),
            ),
        }
    }
}

impl Track {
    pub fn new() -> Self {
        Self {
            rms: Rms::new(RMS_WINDOW_SIZE),
            rms_out: Arc::new([
                AtomicF64::new(-f64::INFINITY),
                AtomicF64::new(-f64::INFINITY),
            ]),
            params: Arc::new(TrackParams::new()),
        }
    }
}

impl Plugin for Track {
    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn send_event(&mut self, _event: PluginEvent) {}

    fn process(&mut self, ctx: &mut ProcessContext) -> ProcessStatus {
        for mut frame in ctx.buffers() {
            let volume = self.params.volume.value() as f32;
            let mute = self.params.mute.value() as f32;
            let output = *frame.input * volume * mute;
            self.rms.add_frame(output);
            frame.write(output);
        }
        let v = self.rms.value().to_db();
        self.rms_out[0].store(v.channel(0) as f64, Ordering::Relaxed);
        self.rms_out[1].store(v.channel(1) as f64, Ordering::Relaxed);

        ProcessStatus::Continue
    }
}

struct Node {
    inner: Option<Box<dyn Plugin + Send>>,
    status: Option<ProcessStatus>,
    deleted: bool,
    mix: Param,
}

impl Node {
    fn new() -> Self {
        Self {
            status: None,
            deleted: false,
            inner: None,
            mix: Param::new(
                1.0,
                ParamInfo::new("Mix", 0, 1).with_smoothing(params::Smoothing::exp_default()),
            ),
        }
    }

    fn send_event(&mut self, ev: PluginEvent) {
        if self.deleted {
            return;
        }
        let Some(inner) = &mut self.inner else { return };
        inner.send_event(ev);
        self.status = None;
    }

    fn delete(&mut self) {
        self.deleted = true;
        self.mix.set(0.0);
    }

    fn is_quiet(&self) -> bool {
        self.mix.value() == 0.0
    }

    fn is_idle(&self) -> bool {
        matches!(self.status, Some(ProcessStatus::Idle))
    }

    fn reset(&mut self) -> Box<dyn Plugin + Send> {
        self.deleted = false;
        self.mix.set(1.0);
        self.status = None;
        self.inner.take().unwrap()
    }
}

pub enum ProcessStatus {
    Continue,
    Idle,
}

/// Data passed to a device for processing a single audio buffer
pub struct ProcessContext<'a> {
    pub num_frames: usize,

    mix: Option<&'a Param>,

    buffer_indices: Option<(usize, usize)>,
    buffers: &'a mut [Buffer],
}

impl<'a> ProcessContext<'a> {
    pub fn new(buffers: &'a mut [Buffer], num_frames: usize) -> Self {
        Self {
            num_frames,
            buffers,
            buffer_indices: None,
            mix: None,
        }
    }

    pub fn output(&mut self, idx: usize, range: &Range<usize>) -> impl Iterator<Item = FrameRef> {
        let buf = &mut self.buffers[idx];
        buf[range.clone()].iter_mut().map(|o| {
            let mix = self.mix.map_or(1.0, |v| v.value() as f32);
            FrameRef::new(&Stereo::ZERO, o, mix)
        })
    }

    pub fn buffers(&mut self) -> impl Iterator<Item = FrameRef> {
        let (input, output) = self.buffer_indices.unwrap();

        let [input, output] = GetManyMutExt::get_many_mut(self.buffers, [input, output])
            .expect("buffers should exist");

        let input = input[..self.num_frames].iter();
        let output = output[..self.num_frames].iter_mut();

        iter::zip(input, output).map(|(i, o)| {
            let mix = self.mix.map_or(1.0, |v| v.value() as f32);
            FrameRef::new(i, o, mix)
        })
    }
}

pub struct FrameRef<'a> {
    mix: f32,
    pub input: &'a Stereo,
    output: &'a mut Stereo,
}

impl<'a> FrameRef<'a> {
    fn new(input: &'a Stereo, output: &'a mut Stereo, mix: f32) -> Self {
        Self { input, output, mix }
    }

    pub fn write(&mut self, frame: Stereo) {
        let output = frame * self.mix;
        let input = *self.input * (1.0 - self.mix);
        *self.output += input + output;
    }
}

pub trait Plugin {
    fn process(&mut self, ctx: &mut ProcessContext) -> ProcessStatus;
    fn params(&self) -> Arc<dyn Params>;
    fn send_event(&mut self, event: PluginEvent);
}

#[derive(Clone, Copy)]
pub struct PluginEvent {
    /// offset of the event within the audio buffer
    pub offset: usize,
    pub track_idx: usize,
    pub note: Note,
}

impl PluginEvent {
    pub fn new(offset: usize, track_idx: usize, note: Note) -> Self {
        Self {
            offset,
            track_idx,
            note,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Pattern {
    /// length of this pattern in ticks
    pub length: usize,
    pub events: Vec<Event>,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub note: Note,
    /// offset in ticks relative to the start of the pattern
    pub offset: usize,
    pub node_index: usize,
    pub track_index: usize,
}

impl Event {
    pub fn new(note: Note, offset: usize, track_index: usize, node_index: usize) -> Self {
        Self {
            note,
            offset,
            node_index,
            track_index,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    On(u8, u8),
    Off,
}
