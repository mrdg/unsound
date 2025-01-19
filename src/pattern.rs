use std::ops::{Add, Sub};

use ratatui::style::Color;

use crate::engine::{Instruction, Note, Sequence};
use crate::{app::random_color, engine::INSTRUMENT_TRACKS, engine::TICKS_PER_LINE};

pub const INPUTS_PER_STEP: usize = 6;
pub const MAX_PITCH: u8 = 109;
pub const NOTE_OFF: u8 = MAX_PITCH;
pub const DEFAULT_VELOCITY: u8 = 100;

const DEFAULT_PATTERN_LEN: usize = 32;
const MAX_PATTERN_LEN: usize = 512;
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

impl Add for Position {
    type Output = Position;
    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.line + rhs.line, self.column + rhs.column)
    }
}

impl Sub for Position {
    type Output = Position;
    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.line - rhs.line, self.column - rhs.column)
    }
}

impl Position {
    fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }

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

#[derive(Default)]
pub struct Rect {
    pub lines: usize,
    pub columns: usize,
}

impl Rect {
    fn new(lines: usize, columns: usize) -> Self {
        Self { lines, columns }
    }
}

#[derive(Clone, Debug)]
pub struct Pattern {
    pub color: Color,
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
        Self {
            color: random_color(),
            tracks,
        }
    }

    pub fn size(&self) -> Rect {
        Rect::new(self.len(), self.tracks.len() * INPUTS_PER_STEP)
    }

    pub fn len(&self) -> usize {
        self.tracks[0].steps.len()
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

    fn step(&self, pos: Position) -> &Step {
        &self.tracks[pos.track()].steps[pos.line]
    }

    fn cell(&self, pos: Position) -> Option<u8> {
        *self.step(pos).cell(pos.input().idx)
    }

    fn cell_mut(&mut self, pos: Position) -> &mut Option<u8> {
        self.step_mut(pos).cell_mut(pos.input().idx)
    }

    pub fn copy(&mut self, start: Position, src: &Pattern, selection: &Selection) {
        let dst_size = self.size();
        let src_size = selection.size();
        if dst_size.lines - start.line < src_size.lines
            || dst_size.columns - start.column < src_size.columns
        {
            // TODO: truncate selection or automatically increase dst pattern size?
            return;
        }

        let src_start = selection.start();

        // Check that src and dst are aligned.
        // TODO: allow pasting from fx1 into fx2 and vice versa
        // TODO: return error if selection can't be copied
        if src_start.input().idx != start.input().idx {
            return;
        }

        for pos in selection.iter() {
            *self.cell_mut(start + pos) = src.cell(src_start + pos);
        }
    }

    pub fn compile(&self) -> Sequence {
        let mut instructions = Vec::new();
        for (i, track) in self.tracks.iter().enumerate() {
            let mut pattern_offset = 0;
            for step in &track.steps {
                let offset = u8::min(TICKS_PER_LINE as u8 - 1, step.offset().unwrap_or(0));
                let note_offset = pattern_offset + offset as usize;
                let instrument = step.instrument().unwrap_or(i as u8);
                let velocity = step.velocity();
                for pitch in step.notes() {
                    let note = if pitch == NOTE_OFF {
                        Note::Off
                    } else {
                        Note::On(pitch, velocity)
                    };
                    let note = Instruction::new(note, note_offset, i, instrument as usize);
                    instructions.push(note);
                }
                pattern_offset += TICKS_PER_LINE;
            }
        }
        instructions.sort_by(|a, b| a.offset.cmp(&b.offset));

        Sequence {
            length: self.len() * TICKS_PER_LINE,
            instructions,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Track {
    steps: Vec<Step>,
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
                if val as usize >= INSTRUMENT_TRACKS {
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

    fn offset(&self) -> Option<u8> {
        self.effects().find(|e| e.cmd == FX_OFFSET).map(|e| e.value)
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

#[derive(Clone)]
pub struct Selection {
    // The cursor position when the selection was started.
    start: Position,
    // The most recent cursor position for this selection. This position is included in the
    // selection.
    end: Position,
}

impl Selection {
    pub fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }

    pub fn move_to(&mut self, pos: Position) {
        self.end = pos;
    }

    pub fn contains(&self, line: usize, column: usize) -> bool {
        let start = self.start();
        let end = self.end();
        start.line <= line && line <= end.line && start.column <= column && column <= end.column
    }

    fn start(&self) -> Position {
        let line = usize::min(self.start.line, self.end.line);
        let col = usize::min(self.start.column, self.end.column);
        Position::new(line, col)
    }

    fn end(&self) -> Position {
        let line = usize::max(self.start.line, self.end.line);
        let col = usize::max(self.start.column, self.end.column);
        Position::new(line, col)
    }

    fn size(&self) -> Rect {
        let start = self.start();
        let end = self.end();
        Rect::new(end.line - start.line + 1, end.column - start.column + 1)
    }

    fn iter(&self) -> impl Iterator<Item = Position> {
        SelectionIter {
            curr: Position::new(0, 0),
            end: self.end() - self.start(),
        }
    }
}

// Iterates over the points in a selection. The points it yields are relative to the start of the
// selection, to make it easier to calculate the destination point when copy/pasting.
struct SelectionIter {
    curr: Position,
    end: Position,
}

impl Iterator for SelectionIter {
    type Item = Position;
    fn next(&mut self) -> Option<Self::Item> {
        if self.curr.column > self.end.column {
            return None;
        }
        let curr = &mut self.curr;
        let pos = *curr;
        curr.line += 1;
        if curr.line > self.end.line {
            curr.line = 0;
            curr.column += 1;
        }
        Some(pos)
    }
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

    fn collect_events(tick: usize, pattern: &Pattern) -> Vec<Instruction> {
        let mut events = Vec::new();
        for event in pattern.compile().instructions {
            if event.offset == tick {
                events.push(event);
            }
        }
        events
    }

    #[test]
    fn note_on_line() {
        let mut pattern = Pattern::new(2);
        for track in 0..2 {
            let step = pattern.step_mut(pos(track, 0));
            *step = Step::default().pitch(60).into();
        }

        let notes = collect_events(0, &pattern);
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
        let notes = collect_events(0, &pattern);
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
        let notes = collect_events(2, &pattern);
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
        let notes = collect_events(11, &pattern);
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
        let notes = collect_events(0, &pattern);
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

        let notes = collect_events(2, &pattern);
        assert_eq!(1, notes.len());

        // The second offset is discarded
        let notes = collect_events(3, &pattern);
        assert_eq!(0, notes.len());
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

        let notes = collect_events(0, &pattern);
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

        let notes = collect_events(0, &pattern);
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

        let notes = collect_events(0, &pattern);
        assert_eq!(1, notes.len());
    }

    #[test]
    fn selection_iter() {
        let a = Position::new(2, 2);
        let b = Position::new(3, 3);
        let s = Selection::new(a, b);
        let expected = vec![
            Position::new(0, 0),
            Position::new(1, 0),
            Position::new(0, 1),
            Position::new(1, 1),
        ];
        assert_eq!(expected, s.iter().collect::<Vec<Position>>());
    }

    #[test]
    fn selection_length_exceeds_pattern_length() {
        let mut p1 = Pattern::new(1);
        p1.set_len(16);
        let p2 = p1.clone();

        let s = Selection::new(Position::new(0, 0), Position::new(8, 0));
        // No assertions but this panics without the bounds checking
        p1.copy(Position::new(8, 0), &p2, &s);
    }

    #[test]
    fn selection_width_exceeds_pattern_width() {
        let mut p1 = Pattern::new(2);
        p1.set_len(16);
        let p2 = p1.clone();

        let s = Selection::new(Position::new(0, 0), Position::new(0, 11));
        p1.copy(Position::new(0, 6), &p2, &s);
    }
}
