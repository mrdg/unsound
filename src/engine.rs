use std::iter;
use std::ops::Range;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use atomic_float::AtomicF64;
use ringbuf::{Consumer, Producer};
use slotmap::SecondaryMap;
use triple_buffer::Input;

use crate::app::{AppCommand, AppState, EngineState};
use crate::audio::{self, Rms, Stereo};
use crate::audio_graph::{self, NodeId};
use crate::params::{self, Param, ParamInfo, Params};
use crate::sampler::{Sampler, Sound};
use crate::SAMPLE_RATE;
use param_derive::Params;

pub const TICKS_PER_LINE: usize = 12;
const RMS_WINDOW_SIZE: usize = SAMPLE_RATE as usize / 10 * 3;
const SUBFRAMES_PER_SEC: usize = 282240000; // LCM of common sample rates

type BufferMap = SecondaryMap<NodeId, Buffer>;

pub enum EngineCommand {
    CreateNode(NodeId, Node),
    CreateBuffer(NodeId, Buffer),
    DeleteNode(NodeId, DeleteMode),
    ForceDeleteNode(NodeId),
    PreviewSound(NodeId, Arc<Sound>),
    NoteEvent(Event),
}

pub struct Engine {
    state: EngineState,
    state_buf: Input<EngineState>,

    nodes: SecondaryMap<NodeId, Node>,
    buffers: SecondaryMap<NodeId, Buffer>,

    consumer: Consumer<EngineCommand>,
    producer: Producer<AppCommand>,

    /// Number of subframes until the next tick
    subframe_countdown: usize,
    total_ticks: u64,

    preview: Sampler,

    discard: Buffer,
}

impl Engine {
    pub fn new(
        state: EngineState,
        state_buf: Input<EngineState>,
        consumer: Consumer<EngineCommand>,
        producer: Producer<AppCommand>,
    ) -> Engine {
        let nodes = SecondaryMap::with_capacity(audio_graph::DEFAULT_SIZE);
        let buffers = SecondaryMap::with_capacity(audio_graph::DEFAULT_SIZE);
        let preview = Sampler::new(Sound::silence());

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
            discard: Buffer::default(),
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

        for entry in &state.process_schedule.entries {
            // Nodes are deleted in the engine before they're deleted from the graph
            // so the schedule can contain node ids for deleted nodes.
            if !self.nodes.contains_key(entry.node_id) {
                continue;
            }
            let node = &mut self.nodes[entry.node_id];

            if node.is_idle() {
                continue;
            }
            let mut ctx = ProcessContext::new(&mut self.buffers, &mut self.discard, frames);
            ctx.volume = Some(&node.volume);
            ctx.mix = Some(&node.mix);

            if let (Some(input), Some(output)) = (entry.input_buffer, entry.output_buffer) {
                ctx.buffer_indices = Some((input, output));
            }
            node.status = Some(node.inner.process(&mut ctx));
            if let Some(input) = entry.input_buffer {
                for frame in &mut self.buffers[input].frames {
                    *frame = Stereo::ZERO;
                }
            }

            if node.should_drop() {
                let node = self.nodes.remove(entry.node_id).unwrap();
                self.producer
                    .push(AppCommand::DropNode(entry.node_id, node))
                    .ok()
                    .unwrap();
            }
        }
        let mut ctx = ProcessContext::new(&mut self.buffers, &mut self.discard, frames);
        self.preview.process(&mut ctx);

        if let Some(i) = state.process_schedule.main_output_buffer {
            let main = &mut self.buffers[i].frames[..frames];
            for (i, frame) in main.iter_mut().enumerate() {
                buffer[i] = *frame;
                *frame = Stereo::ZERO;
            }
        }

        for (_, buf) in self.buffers.iter_mut() {
            for frame in &mut buf.frames {
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
            if !self.buffers.contains_key(event.buffer) {
                continue;
            }
            if !self.nodes.contains_key(event.node) {
                continue;
            }
            if event.offset == self.state.current_tick {
                let node_id = event.node;
                let buffer_id = event.buffer;

                let buf = &mut self.buffers[buffer_id];
                if let Some((tick, node_id)) = buf.previous_event {
                    if tick != self.total_ticks && self.nodes.contains_key(node_id) {
                        let node = &mut self.nodes[node_id];
                        node.send_event(PluginEvent::new(offset, buffer_id, Note::Off));
                    }
                }

                buf.previous_event = Some((self.total_ticks, node_id));
                if let Note::Off = event.note {
                    buf.previous_event = None;
                }

                let node = &mut self.nodes[node_id];
                node.send_event(PluginEvent::new(offset, buffer_id, event.note));
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
                EngineCommand::NoteEvent(event) => {
                    let node = &mut self.nodes[event.node];
                    node.send_event(PluginEvent::new(event.offset, event.buffer, event.note));
                }
                EngineCommand::CreateNode(node_id, node) => {
                    self.nodes.insert(node_id, node);
                }
                EngineCommand::CreateBuffer(node_id, buffer) => {
                    self.buffers.insert(node_id, buffer);
                }
                EngineCommand::ForceDeleteNode(node_id) => {
                    let msg = if let Some(node) = self.nodes.remove(node_id) {
                        AppCommand::DropNode(node_id, node)
                    } else {
                        let buffer = self.buffers.remove(node_id).unwrap();
                        AppCommand::DropBuffer(node_id, buffer)
                    };
                    self.producer.push(msg).ok().unwrap();
                }
                EngineCommand::DeleteNode(node_idx, delete_mode) => {
                    let node = &mut self.nodes[node_idx];
                    node.delete(delete_mode);
                }
                EngineCommand::PreviewSound(output, sound) => {
                    let velocity = 80; // TODO: handle this with gain instead?
                    self.preview
                        .send_event(PluginEvent::new(0, output, Note::Off));
                    self.preview.load(sound);
                    self.preview
                        .send_event(PluginEvent::new(0, output, Note::On(48, velocity)));
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
}

impl TrackParams {
    fn new() -> Self {
        Self {
            volume: Param::new(
                -6.0,
                ParamInfo::new("Volume", -60.0, 3.0)
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
            let volume = self.params.volume.value();
            let mute = self.params.mute.value();
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

pub struct Node {
    inner: Box<dyn Plugin + Send>,
    status: Option<ProcessStatus>,
    deleted: bool,
    volume: Param,
    mix: Param,
}

impl Node {
    pub fn new(inner: Box<dyn Plugin + Send>) -> Self {
        Self {
            status: None,
            deleted: false,
            inner,
            volume: Param::new(
                1.0,
                ParamInfo::new("Volume", 0.0, 1.0).with_smoothing(params::Smoothing::exp_default()),
            ),
            mix: Param::new(
                1.0,
                ParamInfo::new("Mix", 0.0, 1.0).with_smoothing(params::Smoothing::exp_default()),
            ),
        }
    }

    fn send_event(&mut self, ev: PluginEvent) {
        if self.deleted {
            return;
        }
        self.inner.send_event(ev);
        self.status = Some(ProcessStatus::Continue);
    }

    fn delete(&mut self, mode: DeleteMode) {
        self.deleted = true;
        match mode {
            DeleteMode::Passthrough => self.mix.set(0.0),
            DeleteMode::FadeOut => self.volume.set(0.0),
        }
    }

    fn should_drop(&self) -> bool {
        if !self.deleted {
            return false;
        }
        self.status.is_none()
            || self.is_idle()
            || self.volume.value() == 0.0
            || self.mix.value() == 0.0
    }

    fn is_idle(&self) -> bool {
        matches!(self.status, Some(ProcessStatus::Idle))
    }
}

pub struct Buffer {
    pub frames: audio::Buffer,

    /// Time and node id for the last note-on event for this buffer. Whenever
    /// a note-on is received for this buffer, we send a note-off to previous node.
    previous_event: Option<(u64, NodeId)>,
}

impl Default for Buffer {
    fn default() -> Self {
        Self {
            frames: audio::buffer(),
            previous_event: None,
        }
    }
}

#[derive(Debug)]
pub enum ProcessStatus {
    Continue,
    Idle,
}

/// Data passed to a device for processing a single audio buffer
pub struct ProcessContext<'a> {
    pub num_frames: usize,

    volume: Option<&'a Param>,
    mix: Option<&'a Param>,

    buffer_indices: Option<(NodeId, NodeId)>,
    buffers: &'a mut BufferMap,

    discard: &'a mut Buffer,
}

impl<'a> ProcessContext<'a> {
    pub fn new(buffers: &'a mut BufferMap, discard: &'a mut Buffer, num_frames: usize) -> Self {
        Self {
            num_frames,
            buffers,
            buffer_indices: None,
            volume: None,
            mix: None,
            discard,
        }
    }

    pub fn output(
        &mut self,
        buffer: NodeId,
        range: &Range<usize>,
    ) -> impl Iterator<Item = FrameRef> {
        // Instruments might have active voices associated with a buffer that's
        // been deleted, so fall back to a discard buffer here to allow those voices
        // to process until they're back in the idle state.
        let buf = self.buffers.get_mut(buffer).unwrap_or(self.discard);

        buf.frames[range.clone()].iter_mut().map(|o| {
            let volume = self.volume.map_or(1.0, |v| v.value());
            FrameRef::new(&Stereo::ZERO, o, 1.0, volume)
        })
    }

    pub fn buffers(&mut self) -> impl Iterator<Item = FrameRef> {
        let (input, output) = self.buffer_indices.unwrap();

        let [input, output] = self
            .buffers
            .get_disjoint_mut([input, output])
            .expect("buffers should exist");

        let input = input.frames[..self.num_frames].iter();
        let output = output.frames[..self.num_frames].iter_mut();

        iter::zip(input, output).map(|(i, o)| {
            let volume = self.volume.map_or(1.0, |v| v.value());
            let mix = self.mix.map_or(1.0, |v| v.value());
            FrameRef::new(i, o, mix, volume)
        })
    }
}

pub struct FrameRef<'a> {
    volume: f32,
    mix: f32,
    pub input: &'a Stereo,
    output: &'a mut Stereo,
}

impl<'a> FrameRef<'a> {
    fn new(input: &'a Stereo, output: &'a mut Stereo, mix: f32, volume: f32) -> Self {
        Self {
            input,
            output,
            mix,
            volume,
        }
    }

    pub fn write(&mut self, frame: Stereo) {
        let output = frame * self.mix;
        let input = *self.input * (1.0 - self.mix);
        *self.output += (input + output) * self.volume;
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
    pub buffer: NodeId,
    pub note: Note,
}

impl PluginEvent {
    pub fn new(offset: usize, buffer: NodeId, note: Note) -> Self {
        Self {
            offset,
            buffer,
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
    pub node: NodeId,
    pub buffer: NodeId,
}

impl Event {
    pub fn new(note: Note, offset: usize, buffer: NodeId, node: NodeId) -> Self {
        Self {
            note,
            offset,
            node,
            buffer,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    On(u8, u8),
    Off,
}

pub enum DeleteMode {
    Passthrough,
    FadeOut,
}
