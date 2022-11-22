use crate::app::{
    AppState, Device, DeviceId, EngineState, Instrument, Msg, PatternId, Track, TrackType,
};
use crate::engine::TICKS_PER_LINE;
use crate::files::FileBrowser;
use crate::params::Params;
use crate::pattern::{Pattern, Step};

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

#[derive(Copy, Clone)]
pub struct ViewContext<'a> {
    device_params: &'a HashMap<DeviceId, Arc<dyn Params>>,
    app_state: &'a AppState,
    engine_state: &'a EngineState,
    pub file_browser: &'a FileBrowser,
}

impl<'a> ViewContext<'a> {
    pub fn new(
        device_params: &'a HashMap<DeviceId, Arc<dyn Params>>,
        app_state: &'a AppState,
        engine_state: &'a EngineState,
        file_browser: &'a FileBrowser,
    ) -> ViewContext<'a> {
        Self {
            device_params,
            app_state,
            engine_state,
            file_browser,
        }
    }

    fn app(&self) -> &'a AppState {
        self.app_state
    }

    pub fn lines_per_beat(&self) -> u16 {
        self.app().lines_per_beat
    }

    pub fn bpm(&self) -> u16 {
        self.app().bpm
    }

    pub fn is_playing(&self) -> bool {
        self.app().is_playing
    }

    pub fn instruments(&self) -> &Vec<Option<Instrument>> {
        &self.app().instruments
    }

    pub fn tracks(&self) -> &Vec<Track> {
        &self.app().tracks
    }

    pub fn params(&self, track_idx: usize) -> &Arc<dyn Params> {
        let device = &self.app_state.instruments[track_idx].as_ref().unwrap().id;
        self.device_params.get(device).unwrap()
    }

    pub fn octave(&self) -> u16 {
        self.app_state.octave
    }

    pub fn update_pattern<F>(&self, f: F) -> Msg
    where
        F: Fn(&mut Pattern),
    {
        let mut pattern = self.selected_pattern().clone();
        f(&mut pattern);
        Msg::UpdatePattern(self.selected_pattern_id(), pattern)
    }

    pub fn devices(&self, track_idx: usize) -> &Vec<Device> {
        &self.app_state.tracks[track_idx].effects
    }

    pub fn current_line(&self) -> usize {
        self.engine_state.current_tick / TICKS_PER_LINE
    }

    pub fn active_pattern_index(&self) -> usize {
        self.engine_state.current_pattern
    }

    pub fn master_bus(&self) -> TrackView {
        let track = self.tracks().last().unwrap();
        TrackView {
            track,
            index: self.app_state.tracks.len() - 1,
        }
    }

    pub fn iter_tracks(&self) -> impl Iterator<Item = TrackView> {
        self.tracks()
            .iter()
            .enumerate()
            .map(|(i, track)| TrackView { track, index: i })
    }

    pub fn pattern_steps(&self, track_idx: usize, range: &Range<usize>) -> &[Step] {
        let pattern = self.selected_pattern();
        let steps = pattern.steps(track_idx);
        &steps[range.start..range.end]
    }

    pub fn song(&self) -> &Vec<PatternId> {
        &self.app().song
    }

    pub fn song_iter(&self) -> impl Iterator<Item = &Arc<Pattern>> {
        self.app()
            .song
            .iter()
            .map(|id| self.app().patterns.get(id).unwrap())
    }

    pub fn loop_contains(&self, idx: usize) -> bool {
        if let Some(loop_range) = self.app().loop_range {
            loop_range.0 <= idx && idx <= loop_range.1
        } else {
            false
        }
    }

    pub fn selected_pattern(&self) -> &Pattern {
        let id = self.app().song[self.app().selected_pattern];
        self.app().patterns.get(&id).unwrap()
    }

    pub fn selected_pattern_id(&self) -> PatternId {
        self.app().song[self.app().selected_pattern]
    }

    pub fn selected_pattern_index(&self) -> usize {
        self.app().selected_pattern
    }
}

pub struct TrackView<'a> {
    track: &'a Track,
    pub index: usize,
}

impl TrackView<'_> {
    pub fn name(&self) -> String {
        self.track
            .name
            .clone() // TODO: prevent clone
            .unwrap_or_else(|| self.index.to_string())
    }

    pub fn rms(&self) -> (f32, f32) {
        self.track.rms()
    }

    pub fn is_muted(&self) -> bool {
        self.track.muted
    }

    pub fn volume(&self) -> f64 {
        self.track.volume.db()
    }

    pub fn is_bus(&self) -> bool {
        matches!(self.track.track_type, TrackType::Bus)
    }
}
