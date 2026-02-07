use engine::{Command, Event, ProjectSnapshot};

use crate::widgets::preview::PreviewState;
use crate::widgets::timeline::{TimelineInteraction, TimelineRenderModel, build_render_model};

/// UI message consumed by update.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Engine(Event),
    TimelineScrubbed(i64),
    TimelineSplitRequested,
}

impl Message {
    /// Converts a timeline widget interaction into an app message.
    pub fn from_timeline(interaction: TimelineInteraction) -> Self {
        match interaction {
            TimelineInteraction::Scrubbed(t_tl) => Self::TimelineScrubbed(t_tl),
            TimelineInteraction::SplitRequested => Self::TimelineSplitRequested,
        }
    }
}

/// UI state for the MVP editor screen.
#[derive(Debug, Clone, Default)]
pub struct AppState {
    snapshot: Option<ProjectSnapshot>,
    playhead_tl: i64,
    preview: PreviewState,
}

impl AppState {
    /// Creates an empty app state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one UI message and returns outgoing engine commands.
    pub fn update(&mut self, message: Message) -> Vec<Command> {
        match message {
            Message::Engine(event) => self.apply_engine_event(event),
            Message::TimelineScrubbed(t_tl) => {
                self.playhead_tl = t_tl.max(0);
                vec![Command::SetPlayhead {
                    t_tl: self.playhead_tl,
                }]
            }
            Message::TimelineSplitRequested => vec![Command::Split {
                at_tl: self.playhead_tl,
            }],
        }
    }

    /// Returns render data for the timeline.
    pub fn timeline_render_model(&self, width_px: f32) -> Option<TimelineRenderModel> {
        self.snapshot
            .as_ref()
            .map(|snapshot| build_render_model(snapshot, self.playhead_tl, width_px))
    }

    /// Returns a read-only preview state.
    pub fn preview_state(&self) -> &PreviewState {
        &self.preview
    }

    fn apply_engine_event(&mut self, event: Event) -> Vec<Command> {
        match event {
            Event::ProjectChanged(snapshot) => {
                self.snapshot = Some(snapshot);
                Vec::new()
            }
            Event::PlayheadChanged { t_tl } => {
                self.playhead_tl = t_tl;
                Vec::new()
            }
            Event::PreviewFrameReady { frame, .. } => {
                self.preview.push_frame(frame);
                Vec::new()
            }
            Event::ExportProgress { .. } | Event::ExportFinished { .. } | Event::Error(_) => {
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use engine::api::SegmentSummary;
    use engine::{Command, Event, PreviewFrame, PreviewPixelFormat, ProjectSnapshot};

    use crate::widgets::timeline::TimelineInteraction;

    use super::{AppState, Message};

    #[test]
    fn scrub_message_is_forwarded_as_set_playhead_command() {
        let mut app = AppState::new();

        let commands = app.update(Message::TimelineScrubbed(321));

        assert_eq!(commands, vec![Command::SetPlayhead { t_tl: 321 }]);
    }

    #[test]
    fn split_message_targets_current_playhead() {
        let mut app = AppState::new();
        app.update(Message::Engine(Event::PlayheadChanged { t_tl: 777 }));

        let commands = app.update(Message::TimelineSplitRequested);

        assert_eq!(commands, vec![Command::Split { at_tl: 777 }]);
    }

    #[test]
    fn preview_frame_ready_updates_preview_state() {
        let mut app = AppState::new();

        app.update(Message::Engine(Event::PreviewFrameReady {
            t_tl: 123,
            frame: PreviewFrame {
                width: 320,
                height: 180,
                format: PreviewPixelFormat::Rgba8,
                bytes: Arc::from(vec![0_u8; 320 * 180 * 4]),
            },
        }));

        let latest = app
            .preview_state()
            .latest()
            .expect("latest preview frame should be present");
        assert_eq!(latest.width, 320);
        assert_eq!(latest.height, 180);
    }

    #[test]
    fn timeline_render_model_uses_project_snapshot_segments() {
        let mut app = AppState::new();
        app.update(Message::Engine(Event::ProjectChanged(ProjectSnapshot {
            assets: Vec::new(),
            segments: vec![SegmentSummary {
                id: 1,
                asset_id: 1,
                timeline_start: 0,
                timeline_duration: 1_000,
                src_in_video: None,
                src_out_video: None,
                src_in_audio: None,
                src_out_audio: None,
            }],
            duration_tl: 1_000,
        })));
        app.update(Message::Engine(Event::PlayheadChanged { t_tl: 500 }));

        let model = app
            .timeline_render_model(200.0)
            .expect("model should be available once snapshot exists");

        assert_eq!(model.segments.len(), 1);
        assert_eq!(model.playhead_x, 100.0);
    }

    #[test]
    fn timeline_interaction_is_converted_to_split_message_flow() {
        let mut app = AppState::new();
        app.update(Message::Engine(Event::PlayheadChanged { t_tl: 88 }));

        let command = app.update(Message::from_timeline(TimelineInteraction::SplitRequested));

        assert_eq!(command, vec![Command::Split { at_tl: 88 }]);
    }
}
