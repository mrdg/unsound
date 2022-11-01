use crate::engine::TICKS_PER_LINE;

pub const INPUTS_PER_STEP: usize = 6;
pub const MAX_PITCH: u8 = 109;
pub const NOTE_OFF: u8 = MAX_PITCH;
pub const MAX_PATTERNS: usize = 256;
pub const DEFAULT_VELOCITY: u8 = 100;

const DEFAULT_PATTERN_LEN: usize = 16;
const MAX_PATTERN_LEN: usize = 512;
const MAX_INSTRUMENT: u8 = 99;
const MAX_VELOCITY: u8 = 127;

const FX_CHORD: char = 'C';
const FX_OFFSET: char = 'O';
const FX_VELOCITY: char = 'V';

const PITCH: usize = 0;
const INSTR: usize = 1;
const FX_CMD1: usize = 2;
const FX_VAL1: usize = 3;
const FX_CMD2: usize = 4;
const FX_VAL2: usize = 5;

#[derive(Clone, Copy, Debug, Default)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

impl Position {
    pub fn track(&self) -> usize {
        self.column / INPUTS_PER_STEP
    }

    fn input(&self) -> Input {
        let i = self.column % INPUTS_PER_STEP;
        use InputKind::*;
        match i {
            PITCH => Input::new(i, Pitch),
            INSTR => Input::new(i, Instr),
            FX_CMD1 | FX_CMD2 => Input::new(i, EffectCmd),
            FX_VAL1 | FX_VAL2 => Input::new(i, EffectVal),
            _ => unreachable!(),
        }
    }

    pub fn is_pitch_input(&self) -> bool {
        self.column % INPUTS_PER_STEP == PITCH
    }
}

#[derive(Clone, Debug)]
pub struct Pattern {
    pub tracks: Vec<Track>,
}

impl Pattern {
    pub fn new(num_tracks: usize) -> Self {
        let mut tracks = Vec::with_capacity(num_tracks);
        for _ in 0..num_tracks {
            tracks.push(Track {
                steps: vec![Step::default(); DEFAULT_PATTERN_LEN],
            })
        }
        Self { tracks }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.len(), self.tracks.len() * INPUTS_PER_STEP)
    }

    pub fn len(&self) -> usize {
        self.tracks[0].steps.len()
    }

    pub fn ticks(&self) -> usize {
        self.len() * TICKS_PER_LINE
    }

    pub fn set_len(&mut self, new_len: usize) {
        if new_len > MAX_PATTERN_LEN {
            // TODO: return error
            return;
        }
        for track in &mut self.tracks {
            track.steps.resize(new_len, Step::default())
        }
    }

    pub fn steps(&self, track_idx: usize) -> &Vec<Step> {
        &self.tracks[track_idx].steps
    }

    pub fn incr(&mut self, pos: Position, step_size: StepSize) {
        let step = self.step_mut(pos);
        step.incr(pos.input(), step_size);
    }

    pub fn decr(&mut self, pos: Position, step_size: StepSize) {
        let step = self.step_mut(pos);
        step.decr(pos.input(), step_size);
    }

    pub fn set_key(&mut self, pos: Position, octave: u8, key: char) {
        let step = self.step_mut(pos);
        step.set_key(pos.input(), octave, key);
    }

    pub fn clear(&mut self, pos: Position) {
        let step = self.step_mut(pos);
        step.clear(pos.input())
    }

    fn step_mut(&mut self, pos: Position) -> &mut Step {
        &mut self.tracks[pos.track()].steps[pos.line]
    }

    // For each track in the pattern, return notes that should be played on the given tick. The
    // tick is relative to the start of the pattern.
    pub fn events(&self, tick: usize) -> impl Iterator<Item = NoteEvent> + '_ {
        let line = tick / TICKS_PER_LINE;
        self.tracks.iter().enumerate().flat_map(move |(i, track)| {
            let step = &track.steps[line];
            let line_tick = line * TICKS_PER_LINE;

            let mut has_offset = false;
            let offset_match = step.offsets().any(|offset| {
                has_offset = true;
                line_tick + offset as usize == tick
            });

            // TODO: ensure that instrument is always set when pitch is set (it will use the
            // instrument you have selected in the instrument list). Otherwise the behavior
            // here becomes inconsistent when editing the instrument list.
            let instrument = step.instrument().unwrap_or(i as u8) as usize;
            let velocity = step.velocity();

            let notes = if (!has_offset && line_tick == tick) || offset_match {
                let iter = step.notes().map(move |pitch| {
                    let note = if pitch == NOTE_OFF {
                        Note::Off
                    } else {
                        Note::On(pitch, velocity)
                    };
                    NoteEvent {
                        note,
                        track: i,
                        instrument,
                    }
                });
                Some(iter)
            } else {
                None
            };
            notes.into_iter().flatten()
        })
    }
}

#[derive(Clone, Debug)]
pub struct Track {
    steps: Vec<Step>,
}

#[derive(Clone)]
pub struct NoteEvent {
    pub note: Note,
    pub instrument: usize,
    pub track: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    On(u8, u8),
    Off,
}

#[derive(Copy, Clone)]
struct Input {
    idx: usize,
    kind: InputKind,
}

impl Input {
    fn new(idx: usize, kind: InputKind) -> Self {
        Self { idx, kind }
    }
}

#[derive(Copy, Clone)]
enum InputKind {
    Pitch,
    Instr,
    EffectCmd,
    EffectVal,
}

#[derive(Clone, Debug, Default)]
pub struct Step {
    cells: [Option<u8>; INPUTS_PER_STEP],
}

impl Step {
    fn set_key(&mut self, input: Input, octave: u8, key: char) {
        use InputKind::*;
        let val = match input.kind {
            Pitch => key_to_pitch(octave, key),
            Instr | EffectVal => match (self.cell(input.idx), key.to_digit(10)) {
                (Some(val), Some(d)) => {
                    let d = d as i16;
                    let val = *val as i16 * 10 + d;
                    if val <= u8::MAX.into() {
                        Some(val as u8)
                    } else {
                        None
                    }
                }
                (None, Some(d)) => Some(d as u8),
                _ => None,
            },
            _ => Some(key as u8),
        };

        if let Some(val) = val {
            self.set(input, val);
        }
    }

    fn incr(&mut self, input: Input, step_size: StepSize) {
        let step = step_size.for_input(input);
        if let Some(v) = self.cell(input.idx) {
            self.set(input, v.saturating_add(step));
        }
    }

    fn decr(&mut self, input: Input, step_size: StepSize) {
        let step = step_size.for_input(input);
        if let Some(v) = self.cell(input.idx) {
            self.set(input, v.saturating_sub(step));
        }
    }

    fn clear(&mut self, input: Input) {
        *self.cell_mut(input.idx) = None;
    }

    fn set(&mut self, input: Input, val: u8) {
        use InputKind::*;
        match input.kind {
            Pitch => {
                if val > MAX_PITCH {
                    return;
                }
            }
            Instr => {
                if val > MAX_INSTRUMENT {
                    return;
                }
            }
            EffectCmd => {
                if !(val as char).is_ascii_alphabetic() {
                    return;
                }
            }
            EffectVal => {}
        }
        *self.cell_mut(input.idx) = Some(val);
    }

    fn cell_mut(&mut self, idx: usize) -> &mut Option<u8> {
        &mut self.cells[idx]
    }

    fn cell(&self, idx: usize) -> &Option<u8> {
        &self.cells[idx]
    }

    pub fn pitch(&self) -> Option<u8> {
        *self.cell(PITCH)
    }

    pub fn instrument(&self) -> Option<u8> {
        *self.cell(INSTR)
    }

    pub fn effect_cmd(&self, idx: usize) -> Option<u8> {
        assert!(idx < 2);
        *self.cell(FX_CMD1 + idx * 2)
    }

    pub fn effect_val(&self, idx: usize) -> Option<u8> {
        assert!(idx < 2);
        *self.cell(FX_VAL1 + idx * 2)
    }

    fn notes(&self) -> impl Iterator<Item = u8> {
        let chord = if let Some(chord) = self.chord() {
            let iter = ChordIter {
                root: self.pitch(),
                chord,
            };
            Some(iter)
        } else {
            None
        };
        self.pitch().into_iter().chain(chord.into_iter().flatten())
    }

    fn chord(&self) -> Option<u8> {
        self.effects().find(|e| e.cmd == FX_CHORD).map(|e| e.value)
    }

    fn velocity(&self) -> u8 {
        self.effects()
            .find(|e| e.cmd == FX_VELOCITY)
            .map(|e| u8::min(MAX_VELOCITY, e.value))
            .unwrap_or(DEFAULT_VELOCITY)
    }

    fn offsets(&self) -> impl Iterator<Item = u8> + '_ {
        self.effects().flat_map(|e| {
            if e.cmd == FX_OFFSET {
                Some(u8::min(e.value, TICKS_PER_LINE as u8 - 1))
            } else {
                None
            }
        })
    }

    fn effects(&self) -> impl Iterator<Item = Effect> + '_ {
        (0..2).flat_map(move |n| match (self.effect_cmd(n), self.effect_val(n)) {
            (Some(cmd), Some(value)) => Some(Effect {
                cmd: cmd as char,
                value,
            }),
            _ => None,
        })
    }
}

#[derive(Copy, Clone)]
pub enum StepSize {
    Default = 0,
    Large,
}

impl StepSize {
    fn for_input(&self, input: Input) -> u8 {
        match (input.kind, self) {
            (_, StepSize::Default) => 1,
            (InputKind::Pitch, StepSize::Large) => 12,
            (_, StepSize::Large) => 10,
        }
    }
}

pub struct Effect {
    pub cmd: char,
    pub value: u8,
}

struct ChordIter {
    root: Option<u8>,
    chord: u8,
}

impl Iterator for ChordIter {
    type Item = u8;
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(root) = self.root {
            if root != NOTE_OFF && self.chord > 0 {
                let offset = self.chord % 10;
                self.chord /= 10;
                return Some(root + offset);
            }
        }
        None
    }
}

fn key_to_pitch(octave: u8, key: char) -> Option<u8> {
    let mut pitch = match key {
        'z' => 0,
        's' => 1,
        'x' => 2,
        'd' => 3,
        'c' => 4,
        'v' => 5,
        'g' => 6,
        'b' => 7,
        'h' => 8,
        'n' => 9,
        'j' => 10,
        'm' => 11,
        'a' => NOTE_OFF,
        _ => return None,
    };
    if pitch != NOTE_OFF {
        pitch += octave * 12;
    }
    Some(pitch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(track: usize, line: usize) -> Position {
        Position {
            column: track * INPUTS_PER_STEP,
            line,
        }
    }

    #[derive(Default)]
    struct Effect {
        cmd: Option<u8>,
        val: Option<u8>,
    }

    #[derive(Default)]
    struct Step {
        pitch: Option<u8>,
        instr: Option<u8>,
        effects: [Effect; 2],
    }

    impl Into<super::Step> for Step {
        fn into(self) -> super::Step {
            super::Step {
                cells: [
                    self.pitch,
                    self.instr,
                    self.effects[0].cmd,
                    self.effects[0].val,
                    self.effects[1].cmd,
                    self.effects[1].val,
                ],
            }
        }
    }

    impl Step {
        fn pitch(mut self, pitch: u8) -> Step {
            self.pitch = Some(pitch);
            self
        }

        fn effect_cmd(mut self, idx: usize, cmd: char) -> Step {
            self.effects[idx].cmd = Some(cmd as u8);
            self
        }

        fn effect_val(mut self, idx: usize, val: u8) -> Step {
            self.effects[idx].val = Some(val);
            self
        }
    }

    #[test]
    fn note_on_line() {
        let mut pattern = Pattern::new(2);
        for track in 0..2 {
            let step = pattern.step_mut(pos(track, 0));
            *step = Step::default().pitch(60).into();
        }

        let notes: Vec<NoteEvent> = pattern.events(0).collect();
        let pitches: Vec<u8> = notes
            .iter()
            .map(|n| {
                if let Note::On(pitch, _) = n.note {
                    Some(pitch)
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        assert_eq!(vec![60, 60], pitches);
        let tracks: Vec<usize> = notes.iter().map(|n| n.track).collect();
        assert_eq!(vec![0, 1], tracks);
    }

    #[test]
    fn max_velocity() {
        let mut pattern = Pattern::new(1);
        let step = pattern.step_mut(pos(0, 0));
        *step = Step::default()
            .pitch(60)
            .effect_cmd(0, FX_VELOCITY)
            .effect_val(0, 255)
            .into();
        let notes: Vec<NoteEvent> = pattern.events(0).collect();
        assert_eq!(1, notes.len());
        let ev = notes.first().unwrap();
        assert_eq!(Note::On(60, 127), ev.note);
    }

    #[test]
    fn note_with_offset() {
        let mut pattern = Pattern::new(1);
        let step = pattern.step_mut(pos(0, 0));
        *step = Step::default()
            .pitch(60)
            .effect_cmd(0, FX_OFFSET)
            .effect_val(0, 2)
            .into();
        let notes: Vec<NoteEvent> = pattern.events(2).collect();
        assert_eq!(1, notes.len());
    }

    #[test]
    fn max_offset() {
        let mut pattern = Pattern::new(1);
        let step = pattern.step_mut(pos(0, 0));
        *step = Step::default()
            .pitch(60)
            .effect_cmd(0, FX_OFFSET)
            .effect_val(0, 14)
            .into();
        let notes: Vec<NoteEvent> = pattern.events(11).collect();
        assert_eq!(1, notes.len());
    }

    #[test]
    fn note_with_offset_but_no_match() {
        let mut pattern = Pattern::new(1);
        let step = pattern.step_mut(pos(0, 0));
        *step = Step::default()
            .pitch(60)
            .effect_cmd(0, FX_OFFSET)
            .effect_val(0, 2)
            .into();
        let notes: Vec<NoteEvent> = pattern.events(0).collect();
        assert_eq!(0, notes.len());
    }

    #[test]
    fn note_with_two_offsets() {
        let mut pattern = Pattern::new(1);
        let step = pattern.step_mut(pos(0, 0));
        *step = Step::default()
            .pitch(60)
            .effect_cmd(0, FX_OFFSET)
            .effect_val(0, 2)
            .effect_cmd(1, FX_OFFSET)
            .effect_val(1, 3)
            .into();

        let notes: Vec<NoteEvent> = pattern.events(2).collect();
        assert_eq!(1, notes.len());

        let notes: Vec<NoteEvent> = pattern.events(3).collect();
        assert_eq!(1, notes.len());
    }

    #[test]
    fn chord() {
        let mut pattern = Pattern::new(1);
        let step = pattern.step_mut(pos(0, 0));
        *step = Step::default()
            .pitch(60)
            .effect_cmd(0, FX_CHORD)
            .effect_val(0, 3)
            .into();

        let notes: Vec<NoteEvent> = pattern.events(0).collect();
        assert_eq!(
            vec![Note::On(60, 100), Note::On(63, 100)],
            notes.iter().map(|n| n.note).collect::<Vec<Note>>()
        );
    }

    #[test]
    fn chord_without_pitch() {
        let mut pattern = Pattern::new(1);
        let step = pattern.step_mut(pos(0, 0));
        *step = Step::default()
            .effect_cmd(0, FX_CHORD)
            .effect_val(0, 3)
            .into();

        let notes: Vec<NoteEvent> = pattern.events(0).collect();
        assert_eq!(0, notes.len());
    }

    #[test]
    fn chord_with_note_off() {
        let mut pattern = Pattern::new(1);
        let step = pattern.step_mut(pos(0, 0));
        *step = Step::default()
            .pitch(NOTE_OFF)
            .effect_cmd(0, FX_CHORD)
            .effect_val(0, 3)
            .into();

        let notes: Vec<NoteEvent> = pattern.events(0).collect();
        assert_eq!(1, notes.len());
    }
}
