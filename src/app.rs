use crate::host::Instrument;
use crate::param::{Param, ParamKey};
use crate::sampler::Sampler;
use crate::seq::{Event, Pattern, Slot};
use anyhow::{anyhow, Result};
use ringbuf::{Consumer, Producer, RingBuffer};
use std::collections::HashMap;
use std::{borrow::BorrowMut, cmp};

pub struct HostState {
    updates: Consumer<HostUpdate>,
    client_state: Producer<ClientUpdate>,

    pub track_mapping: Vec<Option<Box<dyn Instrument>>>,
    pub current_pattern: Pattern,
    pub is_playing: bool,
    pub params: HashMap<HostParam, f32>,
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
                HostUpdate::SetParamValue(index, key, value) => {
                    match key {
                        ParamKey::Host(param) => {
                            self.params.insert(param, value);
                        }
                        ParamKey::Sampler(_) => {
                            let index = index.unwrap();
                            let instrument = &mut self.track_mapping[index];
                            match instrument {
                                Some(instr) => instr.set_param(key, value).expect("set param"),
                                None => {}
                            }
                        }
                    };
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

pub enum ParamUpdate {
    Inc,
    Dec,
    Set(String),
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub enum HostParam {
    Bpm,
    Octave,
    LinesPerBeat,
}

pub enum HostUpdate {
    PutInstrument(usize, Box<dyn Instrument>),
    TogglePlay,
    PutPatternEvent { event: Event, slot: Slot },
    SetParamValue(Option<usize>, ParamKey, f32),
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
    params: HashMap<HostParam, Param>,
}

impl ClientState {
    pub fn apply_updates(&mut self) {
        while let Some(update) = self.updates.pop() {
            match update {
                ClientUpdate::SetCurrentLine(line) => self.current_line = line,
            }
        }
    }

    pub fn host_param(&self, param: HostParam) -> f32 {
        self.params.get(&param).unwrap().val
    }

    pub fn take(&mut self, action: Action) -> Result<()> {
        match action {
            Action::Exit => {
                self.is_playing = false;
                self.should_stop = true;
            }
            Action::AddTrack(path) => {
                let sampler = Sampler::with_sample(path.as_str())?;
                self.instruments.push(InstrumentState {
                    name: String::from(path),
                    params: sampler.params(),
                });
                self.update_host(HostUpdate::PutInstrument(0, Box::new(sampler)))?
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
            Action::UpdateParam(index, key, update) => {
                let param = match key {
                    ParamKey::Sampler(_) => {
                        let index = index.unwrap();
                        self.instruments
                            .get_mut(index)
                            .and_then(|instr| instr.params.iter_mut().find(|(k, _)| *k == key))
                            .and_then(|p| Some(p.1.borrow_mut()))
                    }
                    ParamKey::Host(key) => self.params.get_mut(&key),
                };
                if let Some(param) = param {
                    match update {
                        ParamUpdate::Inc => param.inc(),
                        ParamUpdate::Dec => param.dec(),
                        ParamUpdate::Set(s) => {
                            let value = s.parse()?;
                            param.set(value)?
                        }
                    }
                    let val = param.val;
                    self.update_host(HostUpdate::SetParamValue(index, key, val))?;
                }
            }
        }
        Ok(())
    }

    fn update_host(&mut self, update: HostUpdate) -> Result<()> {
        if self.host_state.push(update).is_err() {
            Err(anyhow!("unable to send message to host"))
        } else {
            Ok(())
        }
    }
}

pub enum ClientUpdate {
    SetCurrentLine(usize),
}

pub fn new() -> (HostState, ClientState) {
    let (host_prod, host_cons) = RingBuffer::<HostUpdate>::new(16).split();
    let (client_prod, client_cons) = RingBuffer::<ClientUpdate>::new(16).split();

    let mut params = HashMap::new();
    params.insert(HostParam::Bpm, Param::new(1.0, 120.0, 300.0, 1.0));
    params.insert(HostParam::Octave, Param::new(1.0, 4.0, 6.0, 1.0));
    params.insert(HostParam::LinesPerBeat, Param::new(1.0, 4.0, 16.0, 1.0));

    (
        HostState {
            updates: host_cons,
            client_state: client_prod,
            track_mapping: Vec::with_capacity(32),
            current_pattern: Pattern::new(),
            is_playing: false,
            params: params.iter().map(|(k, v)| (*k, v.val)).collect(),
        },
        ClientState {
            updates: client_cons,
            host_state: host_prod,
            current_line: 0,
            instruments: Vec::new(),
            current_pattern: Pattern::new(),
            is_playing: false,
            should_stop: false,
            params: params,
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
    UpdateParam(Option<usize>, ParamKey, ParamUpdate),
}
