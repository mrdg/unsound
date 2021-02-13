use crate::sampler::ROOT_PITCH;

pub const NUM_TRACK_LANES: usize = 2;
pub const MAX_TRACKS: usize = 8;
pub const MAX_COLS: usize = MAX_TRACKS * NUM_TRACK_LANES;

const MAX_PATTERNS: usize = 32;
const MAX_PATTERN_LENGTH: usize = 512;

#[derive(Clone, Copy, Debug)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

pub enum Move {
    Left,
    Right,
    Up,
    Down,
    Start,
    End,
    Top,
    Bottom,
}

pub struct Editor {
    patterns: Vec<Pattern>,
    edit_index: usize,
    pub cursor: Position,
}

impl Editor {
    pub fn new() -> Self {
        let mut patterns = Vec::with_capacity(MAX_PATTERNS);
        let pattern = Pattern::default();
        patterns.push(pattern);

        Self {
            edit_index: 0,
            patterns,
            cursor: Position { line: 0, column: 0 },
        }
    }

    pub fn current_pattern(&self) -> &Pattern {
        &self.patterns[self.edit_index]
    }

    pub fn num_lines(&self) -> usize {
        self.current_pattern().num_lines
    }

    pub fn selected_track(&self) -> usize {
        self.cursor.column / NUM_TRACK_LANES
    }

    pub fn set_cursor(&mut self, pos: Position) {
        self.cursor = pos;
    }

    pub fn move_cursor(&mut self, m: Move) {
        let height = self.current_pattern().num_lines;
        let cursor = &mut self.cursor;
        let step = 1;
        match m {
            Move::Left if cursor.column == 0 => {}
            Move::Left => cursor.column -= step,
            Move::Right => cursor.column = usize::min(cursor.column + step, MAX_COLS - step),
            Move::Start => cursor.column = 0,
            Move::End => cursor.column = MAX_COLS - step,
            Move::Up if cursor.line == 0 => {}
            Move::Up => cursor.line -= 1,
            Move::Down => cursor.line = usize::min(height - 1, cursor.line + 1),
            Move::Top => cursor.line = 0,
            Move::Bottom => cursor.line = height - 1,
        }
    }

    pub fn set_pitch(&mut self, pitch: u8) {
        let note = self.get_note();
        note.pitch = Some(pitch);
    }

    pub fn set_number(&mut self, num: i32) {
        match self.cursor.column % NUM_TRACK_LANES {
            1 => {
                let note = self.get_note();
                let s = note.sound.get_or_insert(0);
                *s = (*s * 10 + num as u8) % 100;
            }
            _ => {}
        }
    }

    pub fn delete_value(&mut self) {
        let field = self.cursor.column % NUM_TRACK_LANES;
        let note = self.get_note();
        match field {
            0 => note.pitch = None,
            1..=2 => note.sound = None,
            _ => {}
        }
    }

    pub fn change_value(&mut self, delta: i32) {
        let field = self.cursor.column % NUM_TRACK_LANES;
        let note = self.get_note();
        let p = match field {
            0 => note.pitch.get_or_insert(ROOT_PITCH),
            1 => note.sound.get_or_insert(0),
            _ => return,
        };
        let val = *p as i32 + delta;
        *p = i32::max(0, i32::min(val, 127)) as u8;
    }

    fn get_note(&mut self) -> &mut Note {
        let track = self.selected_track();
        let pattern = &mut self.patterns[self.edit_index];
        let track = &mut pattern.tracks[track];
        &mut track.notes[self.cursor.line]
    }

    pub fn iter_tracks(&self) -> impl Iterator<Item = TrackView> {
        let pattern = &self.patterns[self.edit_index];
        pattern.tracks.iter().map(move |track| TrackView {
            notes: &track.notes[0..pattern.num_lines],
        })
    }

    pub fn iter_notes(&self, tick: u64) -> impl Iterator<Item = NoteEvent> + '_ {
        let pattern = &self.patterns[self.edit_index];
        let line = (tick % pattern.num_lines as u64) as usize;
        pattern
            .tracks
            .iter()
            .enumerate()
            .flat_map(move |(i, track)| {
                track
                    .notes
                    .iter()
                    .enumerate()
                    .filter(move |(l, note)| *l == line && note.pitch.is_some())
                    .map(move |(_, &note)| NoteEvent {
                        pitch: note.pitch.unwrap(),
                        track: i as u8,
                        sound: note.sound.unwrap_or(i as u8),
                    })
            })
    }
}

pub struct TrackView<'a> {
    pub notes: &'a [Note],
}

pub struct Pattern {
    pub num_lines: usize,
    tracks: Vec<Track>,
}

impl Default for Pattern {
    fn default() -> Self {
        let mut tracks = Vec::with_capacity(MAX_TRACKS);
        for _ in 0..MAX_TRACKS {
            tracks.push(Track {
                notes: vec![Note::default(); MAX_PATTERN_LENGTH],
            })
        }
        Self {
            num_lines: 32,
            tracks,
        }
    }
}

struct Track {
    notes: Vec<Note>,
}

#[derive(Copy, Clone, Debug)]
pub struct Note {
    pub pitch: Option<u8>,
    pub sound: Option<u8>,
}

impl Default for Note {
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
