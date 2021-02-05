use crate::app::HostState;
use crate::param::ParamKey;
use crate::seq::{Block, Event, Sequencer};
use crate::{FRAMES_PER_BUFFER, SAMPLE_RATE};
use anyhow::Result;
use portaudio::stream_flags as paflags;
use portaudio::{OutputStreamCallbackArgs, PortAudio};
pub trait Instrument {
    fn send_event(&mut self, column: usize, event: &Event);
    fn render(&mut self, buffer: &mut [(f32, f32)]);
    fn set_param(&mut self, key: ParamKey, value: f32) -> Result<()>;
}

pub struct Host {
    stream: portaudio::Stream<portaudio::NonBlocking, portaudio::Output<f32>>,
}

impl Host {
    pub fn run(mut state: HostState) -> Result<Host> {
        let mut buf = [(0., 0.); FRAMES_PER_BUFFER as usize];
        let mut seq = Sequencer::new();
        let pa = PortAudio::new()?;
        let mut settings =
            pa.default_output_stream_settings::<f32>(2, SAMPLE_RATE, FRAMES_PER_BUFFER)?;
        settings.flags = paflags::CLIP_OFF;

        let callback = move |OutputStreamCallbackArgs { buffer, frames, .. }| {
            state.apply_updates();
            let mut block = Block { start: 0, end: 0 };
            while seq.next_block(&mut block, frames, &mut state) {
                for instrument in &mut state.track_mapping {
                    if let Some(instrument) = instrument {
                        instrument.render(&mut buf[block.start..block.end]);
                    }
                }
            }

            let mut i = 0;
            for j in 0..buf.len() {
                buffer[i] = buf[j].0;
                buffer[i + 1] = buf[j].1;
                i += 2;
                buf[j] = (0.0, 0.0);
            }
            portaudio::Continue
        };

        let mut stream = pa.open_non_blocking_stream(settings, callback)?;
        stream.start()?;
        Ok(Host { stream })
    }

    pub fn shutdown(&mut self) -> Result<(), portaudio::Error> {
        self.stream.stop()?;
        self.stream.close()
    }
}
