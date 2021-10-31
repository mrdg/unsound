use crate::engine::EngineCommand;
use crate::pattern::{Pattern, MAX_TRACKS};
use crate::sampler::Sound;
use anyhow::{anyhow, Result};
use basedrop::{Collector, Shared, SharedCell};
use ringbuf::{Consumer, Producer, RingBuffer};
use std::sync::atomic::AtomicU64;
use std::sync::{
    atomic::{AtomicBool, AtomicU16, Ordering},
    Arc,
};

pub struct Store {
    current_tick: AtomicU64,
    sounds: SharedCell<Vec<Option<Shared<Sound>>>>,
    pattern: SharedCell<Pattern>,
    bpm: AtomicU16,
    lines_per_beat: AtomicU16,
    octave: AtomicU16,
    is_playing: AtomicBool,
}

pub trait SharedState {
    fn store(&self) -> &Arc<Store>;

    fn lines_per_beat(&self) -> u16 {
        self.store().lines_per_beat.load(Ordering::Relaxed)
    }

    fn bpm(&self) -> u16 {
        self.store().bpm.load(Ordering::Relaxed)
    }

    fn pattern(&self) -> Shared<Pattern> {
        self.store().pattern.get()
    }

    fn current_tick(&self) -> u64 {
        self.store().current_tick.load(Ordering::Relaxed)
    }
}

pub fn controls() -> (AppControl, EngineControl) {
    let mut sounds = Vec::with_capacity(MAX_TRACKS);
    for _ in 0..MAX_TRACKS {
        sounds.push(None);
    }
    let collector = Collector::new();
    let store = Store {
        sounds: SharedCell::new(Shared::new(&collector.handle(), sounds)),
        current_tick: AtomicU64::new(0),
        pattern: SharedCell::new(Shared::new(&collector.handle(), Pattern::default())),
        bpm: AtomicU16::new(120),
        octave: AtomicU16::new(4),
        lines_per_beat: AtomicU16::new(4),
        is_playing: AtomicBool::new(false),
    };
    let store = Arc::new(store);
    let (producer, consumer) = RingBuffer::<EngineCommand>::new(16).split();
    let app_control = AppControl {
        store: store.clone(),
        producer,
        collector,
    };
    let engine_control = EngineControl { store, consumer };
    (app_control, engine_control)
}

pub struct AppControl {
    store: Arc<Store>,
    producer: Producer<EngineCommand>,
    collector: Collector,
}

impl AppControl {
    pub fn octave(&self) -> u16 {
        self.store.octave.load(Ordering::Relaxed)
    }

    pub fn set_octave(&self, value: u16) {
        self.store.octave.store(value, Ordering::Relaxed)
    }

    pub fn set_bpm(&self, value: u16) {
        self.store.bpm.store(value, Ordering::Relaxed)
    }

    pub fn set_sound(&mut self, idx: usize, snd: Sound) {
        let mut sounds = (*self.store.sounds.get()).clone();
        sounds[idx] = Some(Shared::new(&self.collector.handle(), snd));
        self.store
            .sounds
            .set(Shared::new(&self.collector.handle(), sounds));
    }

    pub fn sounds(&self) -> Shared<Vec<Option<Shared<Sound>>>> {
        self.store.sounds.get()
    }

    pub fn set_lines_per_beat(&self, value: u16) {
        self.store.lines_per_beat.store(value, Ordering::Relaxed)
    }

    pub fn toggle_play(&self) {
        self.store.is_playing.store(
            !self.store.is_playing.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }

    pub fn update_pattern<F>(&mut self, f: F)
    where
        F: Fn(&mut Pattern),
    {
        let mut pattern = (*self.store.pattern.get()).clone();
        f(&mut pattern);
        self.store
            .pattern
            .set(Shared::new(&self.collector.handle(), pattern));
    }

    pub fn preview_sound(&mut self, snd: Sound) -> Result<()> {
        let snd = Shared::new(&self.collector.handle(), snd);
        if self
            .producer
            .push(EngineCommand::PreviewSound(snd))
            .is_err()
        {
            Err(anyhow!("unable to send message to engine"))
        } else {
            Ok(())
        }
    }
}

impl SharedState for AppControl {
    fn store(&self) -> &Arc<Store> {
        &self.store
    }
}

pub struct EngineControl {
    store: Arc<Store>,
    consumer: Consumer<EngineCommand>,
}

impl EngineControl {
    pub fn command(&mut self) -> Option<EngineCommand> {
        self.consumer.pop()
    }

    pub fn is_playing(&self) -> bool {
        self.store.is_playing.load(Ordering::Relaxed)
    }

    pub fn tick(&self) {
        self.store.current_tick.fetch_add(1, Ordering::SeqCst);
    }

    pub fn sound(&self, idx: usize) -> Option<Shared<Sound>> {
        self.store
            .sounds
            .get()
            .get(idx)
            .and_then(|opt| (*opt).clone())
    }
}

impl SharedState for EngineControl {
    fn store(&self) -> &Arc<Store> {
        &self.store
    }
}
