use anyhow::{anyhow, Result};
use std::{
    io,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};
use termion::{event::Key, input::TermRead};

pub enum Input {
    Key(Key),
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
            thread::spawn(move || {
                let stdin = io::stdin();
                for key in stdin.keys().flatten() {
                    sender.send(Input::Key(key)).expect("send keyboard input");
                }
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
