use anyhow::{anyhow, Result};
use crossterm::event::{self, Event};
use std::{
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

pub enum Input {
    Event(Event),
    Tick,
}

pub struct InputQueue {
    events: Receiver<Input>,
}

impl InputQueue {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        {
            let sender = sender.clone();
            thread::spawn(move || loop {
                let event = event::read().expect("event read");
                sender
                    .send(Input::Event(event))
                    .expect("send keyboard input");
            })
        };
        thread::spawn(move || loop {
            if sender.send(Input::Tick).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(33));
        });
        Self { events: receiver }
    }

    pub fn next(&mut self) -> Result<Input> {
        self.events
            .recv()
            .map_err(|err| anyhow!("input receive error: {}", err))
    }
}
