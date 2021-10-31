use crate::sampler::ROOT_PITCH;

pub const NUM_TRACK_LANES: usize = 2;
pub const MAX_TRACKS: usize = 8;
pub const MAX_COLS: usize = MAX_TRACKS * NUM_TRACK_LANES;

const MAX_PATTERN_LENGTH: usize = 512;

#[derive(Clone, Copy, Debug)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

pub struct TrackView<'a> {
    pub steps: &'a [Step],
}

#[derive(Clone, Debug)]
pub struct Pattern {
    pub num_lines: usize,
    tracks: Vec<Track>,
}

impl Default for Pattern {
    fn default() -> Self {
        let mut tracks = Vec::with_capacity(MAX_TRACKS);
        for _ in 0..MAX_TRACKS {
            tracks.push(Track {
                steps: vec![Step::default(); MAX_PATTERN_LENGTH],
            })
        }
        Self {
            num_lines: 32,
            tracks,
        }
    }
}

impl Pattern {
    fn step(&mut self, pos: Position) -> &mut Step {
        let track = pos.column / NUM_TRACK_LANES;
        let track = &mut self.tracks[track];
        &mut track.steps[pos.line]
    }

    pub fn set_pitch(&mut self, pos: Position, pitch: u8) {
        let step = self.step(pos);
        step.pitch = Some(pitch);
    }

    pub fn set_number(&mut self, pos: Position, num: i32) {
        #[allow(clippy::single_match)]
        match pos.column % NUM_TRACK_LANES {
            1 => {
                let step = self.step(pos);
                let s = step.sound.get_or_insert(0);
                *s = ((*s as i32 * 10 + num) % 100) as u8;
            }
            _ => {}
        }
    }

    pub fn delete_value(&mut self, pos: Position) {
        let field = pos.column % NUM_TRACK_LANES;
        let step = self.step(pos);
        match field {
            0 => step.pitch = None,
            1 => step.sound = None,
            _ => {}
        }
    }

    pub fn change_value(&mut self, pos: Position, delta: i32) {
        let field = pos.column % NUM_TRACK_LANES;
        let step = self.step(pos);
        let p = match field {
            0 => step.pitch.get_or_insert(ROOT_PITCH),
            1 => step.sound.get_or_insert(0),
            _ => return,
        };
        let val = *p as i32 + delta;
        *p = i32::max(0, i32::min(val, 127)) as u8;
    }

    pub fn iter_tracks(&self) -> impl Iterator<Item = TrackView> {
        self.tracks.iter().map(move |track| TrackView {
            steps: &track.steps[0..self.num_lines],
        })
    }

    pub fn iter_notes(&self, tick: u64) -> impl Iterator<Item = NoteEvent> + '_ {
        let line = (tick % self.num_lines as u64) as usize;
        self.tracks.iter().enumerate().flat_map(move |(i, track)| {
            track
                .steps
                .iter()
                .enumerate()
                .filter(move |(l, step)| *l == line && step.pitch.is_some())
                .map(move |(_, &step)| NoteEvent {
                    pitch: step.pitch.unwrap(),
                    track: i as u8,
                    sound: step.sound.unwrap_or(i as u8),
                })
        })
    }
}

#[derive(Clone, Debug)]
struct Track {
    steps: Vec<Step>,
}

#[derive(Copy, Clone, Debug)]
pub struct Step {
    pub pitch: Option<u8>,
    pub sound: Option<u8>,
}

impl Default for Step {
    fn default() -> Self {
        Self {
            pitch: None,
            sound: None,
        }
    }
}

pub struct NoteEvent {
    pub pitch: u8,
    pub sound: u8,
    pub track: u8,
}
