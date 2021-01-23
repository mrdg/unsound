use crate::param::{Param, ParamKey};
use crate::sampler::Sampler;
use crate::seq::{Event, Instrument, Pattern, Slot};
use ringbuf::{Consumer, Producer, RingBuffer};
use std::cmp;
use std::error::Error;

pub struct HostState {
    updates: Consumer<HostUpdate>,
    client_state: Producer<ClientUpdate>,

    pub track_mapping: Vec<Option<Box<dyn Instrument>>>,
    pub current_pattern: Pattern,
    pub is_playing: bool,
    pub octave: usize,
    pub lines_per_beat: usize,
    pub bpm: u16,
}

impl HostState {
    pub fn apply_updates(&mut self) {
        while let Some(update) = self.updates.pop() {
            match update {
                HostUpdate::PutInstrument(_, instrument) => {
                    self.track_mapping.push(Some(instrument));
                }
                HostUpdate::TogglePlay => {
                    self.is_playing = !self.is_playing;
                }
                HostUpdate::PutPatternEvent { event, slot } => {
                    self.current_pattern.add_event(event, slot);
                }
                HostUpdate::ParamSetValue(instrument, key, value) => {
                    let instrument = &mut self.track_mapping[instrument];
                    match instrument {
                        Some(instr) => instr.set_param(key, value).expect("set param"),
                        None => {}
                    }
                }
            }
        }
    }

    pub fn set_current_line(&mut self, line: usize) {
        self.update_client(ClientUpdate::SetCurrentLine(line));
    }

    fn update_client(&mut self, update: ClientUpdate) {
        if let Err(_) = self.client_state.push(update) {
            eprintln!("unable to update client state");
        }
    }
}

pub enum HostUpdate {
    PutInstrument(usize, Box<dyn Instrument>),
    TogglePlay,
    PutPatternEvent { event: Event, slot: Slot },
    ParamSetValue(usize, ParamKey, Param),
}

pub struct InstrumentState {
    pub name: String,
    pub params: Vec<(ParamKey, Param)>,
}

pub struct ClientState {
    updates: Consumer<ClientUpdate>,
    host_state: Producer<HostUpdate>,

    pub current_line: usize,
    pub current_pattern: Pattern,
    pub instruments: Vec<InstrumentState>,
    pub is_playing: bool,
    pub should_stop: bool,
    pub octave: i32,
    pub lines_per_beat: usize,
    pub bpm: u16,
}

impl ClientState {
    pub fn apply_updates(&mut self) {
        while let Some(update) = self.updates.pop() {
            match update {
                ClientUpdate::SetCurrentLine(line) => self.current_line = line,
            }
        }
    }

    pub fn take(&mut self, action: Action) -> Result<(), Box<dyn Error>> {
        match action {
            Action::Exit => {
                self.is_playing = false;
                self.should_stop = true;
            }
            Action::AddTrack(path) => {
                self.load_sound(0, path.as_str())?;
            }
            Action::PutNote(slot, pitch) => {
                let pitch = cmp::min(cmp::max(0, pitch), 126);
                let event = Event::NoteOn { pitch };
                self.current_pattern.add_event(event, slot);
                self.update_host(HostUpdate::PutPatternEvent { event, slot })?
            }
            Action::ChangePitch(slot, change) => {
                match self.current_pattern.event_at(slot.line, slot.track) {
                    Event::NoteOn { pitch } => self.take(Action::PutNote(slot, pitch + change))?,
                    _ => {}
                }
            }
            Action::DeleteNote(slot) => {
                let event = Event::Empty;
                self.current_pattern.add_event(event, slot);
                self.update_host(HostUpdate::PutPatternEvent { event, slot })?
            }
            Action::TogglePlay => {
                self.is_playing = !self.is_playing;
                self.update_host(HostUpdate::TogglePlay)?
            }
            Action::ParamUp(instrument, param) => {
                if let Some((key, value)) = self
                    .instruments
                    .get_mut(instrument)
                    .and_then(|instr| instr.params.get_mut(param))
                {
                    value.up();
                    let (key, value) = (*key, *value);
                    self.update_host(HostUpdate::ParamSetValue(instrument, key, value))?
                }
            }
            Action::ParamDown(instrument, param) => {
                if let Some((key, value)) = self
                    .instruments
                    .get_mut(instrument)
                    .and_then(|instr| instr.params.get_mut(param))
                {
                    value.down();
                    let (key, value) = (*key, *value);
                    self.update_host(HostUpdate::ParamSetValue(instrument, key, value))?
                }
            }
        }
        Ok(())
    }

    fn load_sound(&mut self, index: usize, path: &str) -> Result<(), Box<dyn Error>> {
        let sampler = Sampler::with_sample(path)?;
        self.instruments.push(InstrumentState {
            name: String::from(path),
            params: sampler.params(),
        });
        self.update_host(HostUpdate::PutInstrument(index, Box::new(sampler)))
    }

    fn update_host(&mut self, update: HostUpdate) -> Result<(), Box<dyn Error>> {
        self.host_state
            .push(update)
            .map_err(|_| "unable to send message to host".into())
    }
}

pub enum ClientUpdate {
    SetCurrentLine(usize),
}

pub fn new() -> (HostState, ClientState) {
    let (host_prod, host_cons) = RingBuffer::<HostUpdate>::new(16).split();
    let (client_prod, client_cons) = RingBuffer::<ClientUpdate>::new(16).split();
    (
        HostState {
            updates: host_cons,
            client_state: client_prod,
            track_mapping: Vec::with_capacity(32),
            current_pattern: Pattern::new(),
            is_playing: false,
            octave: 4,
            bpm: 120,
            lines_per_beat: 4,
        },
        ClientState {
            updates: client_cons,
            host_state: host_prod,
            current_line: 0,
            instruments: Vec::new(),
            current_pattern: Pattern::new(),
            is_playing: false,
            should_stop: false,
            octave: 4,
            bpm: 120,
            lines_per_beat: 4,
        },
    )
}

pub enum Action {
    Exit,
    AddTrack(String),
    PutNote(Slot, i32),
    DeleteNote(Slot),
    ChangePitch(Slot, i32),
    TogglePlay,
    ParamUp(usize, usize),
    ParamDown(usize, usize),
}
