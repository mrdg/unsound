pub mod app;
pub mod audio;
pub mod delay;
pub mod engine;
pub mod env;
pub mod files;
pub mod input;
pub mod params;
pub mod pattern;
pub mod sampler;
pub mod view;

// Keep https://github.com/RustAudio/cpal/issues/508 in mind
// when changing the sample rate.
pub const SAMPLE_RATE: f64 = 44100.0;
pub const FRAMES_PER_BUFFER: usize = 128;

// Allocate a larger buffer size, because sometimes cpal requests more than the
// configured buffer size when switching the output device.
pub const INTERNAL_BUFFER_SIZE: usize = 4 * FRAMES_PER_BUFFER;
