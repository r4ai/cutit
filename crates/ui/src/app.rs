use std::path::PathBuf;
use std::time::{Duration, Instant};
use std::{cmp, sync::mpsc::TrySendError};

use engine::{Command, EngineErrorKind, Event, ExportSettings, ProjectSnapshot};
use iced::time;
use iced::widget::canvas;
use iced::widget::{button, column, container, row, text, text_input};
use iced::{Element, Length, Subscription, Task};

use crate::bridge::{BridgeEvent, EngineCommandSender, engine_subscription};
use crate::widgets::{preview, timeline};

const IDLE_WARM_MAX_ROUNDS: u16 = 120;

/// UI messages handled by the iced app update loop.
#[derive(Debug, Clone)]
pub enum Message {
    ImportPathChanged(String),
    ImportPressed,
    ExportPathChanged(String),
    ExportPressed,
    PlayPausePressed,
    PlaybackTick(Instant),
    SplitPressed,
    CutPressed,
    TimelineScrubbed(i64),
    TimelineSplitRequested(i64),
    TimelineCutRequested(i64),
    TimelineSegmentMoveRequested { segment_id: u64, new_start_tl: i64 },
    TimelineSegmentTrimStartRequested { segment_id: u64, new_start_tl: i64 },
    TimelineSegmentTrimEndRequested { segment_id: u64, new_end_tl: i64 },
    Bridge(BridgeEvent),
}

/// Root UI state for Step 6 bootstrap.
pub struct AppState {
    engine_tx: Option<EngineCommandSender>,
    project: Option<ProjectSnapshot>,
    preview_image: Option<preview::PreviewImage>,
    import_path: String,
    export_path: String,
    playhead_tl: i64,
    pending_playhead_tl: Option<i64>,
    latest_requested_playhead_tl: Option<i64>,
    playhead_request_in_flight: bool,
    idle_warm_target_tl: Option<i64>,
    idle_warm_rounds: u16,
    loaded_preview_ranges_tl: Vec<(i64, i64)>,
    pending_split_tl: Option<i64>,
    pending_cut_tl: Option<i64>,
    last_split_tl: Option<i64>,
    is_playing: bool,
    timeline_cache: canvas::Cache,
    status: String,
}

impl AppState {
    /// Boots the app and initializes the engine bridge.
    pub fn boot() -> (Self, Task<Message>) {
        (
            Self {
                engine_tx: None,
                project: None,
                preview_image: None,
                import_path: String::new(),
                export_path: String::new(),
                playhead_tl: 0,
                pending_playhead_tl: None,
                latest_requested_playhead_tl: None,
                playhead_request_in_flight: false,
                idle_warm_target_tl: None,
                idle_warm_rounds: 0,
                loaded_preview_ranges_tl: Vec::new(),
                pending_split_tl: None,
                pending_cut_tl: None,
                last_split_tl: None,
                is_playing: false,
                timeline_cache: canvas::Cache::new(),
                status: String::from("starting engine bridge"),
            },
            Task::none(),
        )
    }

    /// Handles one UI message.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ImportPathChanged(path) => {
                self.import_path = path;
            }
            Message::ImportPressed => {
                let path = self.import_path.trim().to_owned();
                if path.is_empty() {
                    self.status = String::from("import path is empty");
                } else if self.send_command(Command::Import {
                    path: PathBuf::from(&path),
                }) {
                    self.pending_split_tl = None;
                    self.pending_cut_tl = None;
                    self.last_split_tl = None;
                    self.status = format!("importing {}", path);
                }
            }
            Message::ExportPathChanged(path) => {
                self.export_path = path;
            }
            Message::ExportPressed => {
                let path = self.export_path.trim().to_owned();
                if path.is_empty() {
                    self.status = String::from("export path is empty");
                } else if self.send_command(Command::Export {
                    path: PathBuf::from(&path),
                    settings: ExportSettings::default(),
                }) {
                    self.status = format!("export requested: {}", path);
                }
            }
            Message::PlayPausePressed => {
                self.toggle_playback();
            }
            Message::PlaybackTick(_at) => {
                self.handle_playback_tick();
            }
            Message::SplitPressed => {
                let clamped = self.clamp_playhead(self.playhead_tl);
                self.playhead_tl = clamped;
                self.request_split(clamped);
                self.queue_playhead_from_user(clamped);
            }
            Message::CutPressed => {
                let clamped = self.clamp_playhead(self.playhead_tl);
                self.playhead_tl = clamped;
                self.request_cut(clamped);
                self.queue_playhead_from_user(clamped);
            }
            Message::TimelineScrubbed(t_tl) => {
                let clamped = self.clamp_playhead(t_tl);
                self.playhead_tl = clamped;
                self.queue_playhead_from_user(clamped);
            }
            Message::TimelineSplitRequested(at_tl) => {
                let clamped = self.clamp_playhead(at_tl);
                self.playhead_tl = clamped;
                self.request_split(clamped);
                self.queue_playhead_from_user(clamped);
            }
            Message::TimelineCutRequested(at_tl) => {
                let clamped = self.clamp_playhead(at_tl);
                self.playhead_tl = clamped;
                self.request_cut(clamped);
                self.queue_playhead_from_user(clamped);
            }
            Message::TimelineSegmentMoveRequested {
                segment_id,
                new_start_tl,
            } => {
                self.request_move_segment(segment_id, new_start_tl);
            }
            Message::TimelineSegmentTrimStartRequested {
                segment_id,
                new_start_tl,
            } => {
                self.request_trim_segment_start(segment_id, new_start_tl);
            }
            Message::TimelineSegmentTrimEndRequested {
                segment_id,
                new_end_tl,
            } => {
                self.request_trim_segment_end(segment_id, new_end_tl);
            }
            Message::Bridge(BridgeEvent::Ready(sender)) => {
                self.engine_tx = Some(sender);
                self.status = String::from("engine ready");
                self.flush_playhead_request();
            }
            Message::Bridge(BridgeEvent::Event(event)) => {
                self.apply_engine_event(event);
            }
            Message::Bridge(BridgeEvent::Disconnected) => {
                self.status = String::from("engine event channel closed");
                self.engine_tx = None;
                self.pending_playhead_tl = None;
                self.latest_requested_playhead_tl = None;
                self.playhead_request_in_flight = false;
                self.idle_warm_target_tl = None;
                self.idle_warm_rounds = 0;
                self.loaded_preview_ranges_tl.clear();
                self.pending_split_tl = None;
                self.pending_cut_tl = None;
                self.last_split_tl = None;
                self.is_playing = false;
            }
        }

        Task::none()
    }

    fn send_command(&mut self, command: Command) -> bool {
        if let Some(sender) = &self.engine_tx {
            match sender.try_send(command) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => {
                    self.status = String::from("engine command queue is full");
                    false
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.status = String::from("engine command channel closed");
                    self.engine_tx = None;
                    self.playhead_request_in_flight = false;
                    false
                }
            }
        } else {
            self.status = String::from("engine is not ready");
            false
        }
    }

    fn queue_playhead(&mut self, t_tl: i64, update_latest: bool) {
        self.pending_playhead_tl = Some(t_tl);
        if update_latest {
            self.latest_requested_playhead_tl = Some(t_tl);
        }
        self.flush_playhead_request();
    }

    fn queue_playhead_from_user(&mut self, t_tl: i64) {
        self.idle_warm_target_tl = None;
        self.idle_warm_rounds = 0;
        self.queue_playhead(t_tl, true);
    }

    fn queue_playhead_for_idle_warm(&mut self, t_tl: i64) {
        self.queue_playhead(t_tl, false);
    }

    fn toggle_playback(&mut self) {
        if self.is_playing {
            self.is_playing = false;
            self.status = String::from("playback paused");
            return;
        }

        if self
            .project
            .as_ref()
            .is_none_or(|snapshot| snapshot.duration_tl <= 0)
        {
            self.status = String::from("cannot start playback: project is empty");
            return;
        }

        let clamped = self.clamp_playhead(self.playhead_tl);
        self.playhead_tl = clamped;
        self.is_playing = true;
        self.status = format!("playback started at {}", clamped);
        self.queue_playhead_from_user(clamped);
    }

    fn handle_playback_tick(&mut self) {
        if !self.is_playing {
            return;
        }

        let Some(project) = self.project.as_ref() else {
            self.is_playing = false;
            self.status = String::from("playback stopped: project is not loaded");
            return;
        };
        if project.duration_tl <= 0 {
            self.is_playing = false;
            self.status = String::from("playback stopped: project is empty");
            return;
        }

        let max_tick = project.duration_tl - 1;
        if self.playhead_tl >= max_tick {
            self.is_playing = false;
            self.status = String::from("playback reached timeline end");
            return;
        }

        let step = self.preview_bucket_tl().max(1);
        let next = self.playhead_tl.saturating_add(step).clamp(0, max_tick);
        if next == self.playhead_tl {
            self.is_playing = false;
            self.status = String::from("playback reached timeline end");
            return;
        }

        self.playhead_tl = next;
        self.queue_playhead_from_user(next);
    }

    fn request_split(&mut self, at_tl: i64) {
        if self.pending_split_tl.is_some() {
            self.status = String::from("split request is already pending");
            return;
        }
        if self.pending_cut_tl.is_some() {
            self.status = String::from("cut request is already pending");
            return;
        }

        if self.send_command(Command::Split { at_tl }) {
            self.pending_split_tl = Some(at_tl);
            self.status = format!("split requested at {}", at_tl);
        }
    }

    fn request_cut(&mut self, at_tl: i64) {
        if self.pending_cut_tl.is_some() {
            self.status = String::from("cut request is already pending");
            return;
        }
        if self.pending_split_tl.is_some() {
            self.status = String::from("split request is already pending");
            return;
        }

        if self.send_command(Command::Cut { at_tl }) {
            self.pending_cut_tl = Some(at_tl);
            self.status = format!("cut requested at {}", at_tl);
        }
    }

    fn request_move_segment(&mut self, segment_id: u64, new_start_tl: i64) {
        if self.send_command(Command::MoveSegment {
            segment_id,
            new_start_tl,
        }) {
            self.status = format!("segment {} moved to {}", segment_id, new_start_tl);
        }
    }

    fn request_trim_segment_start(&mut self, segment_id: u64, new_start_tl: i64) {
        if self.send_command(Command::TrimSegmentStart {
            segment_id,
            new_start_tl,
        }) {
            self.status = format!("segment {} trim-start to {}", segment_id, new_start_tl);
        }
    }

    fn request_trim_segment_end(&mut self, segment_id: u64, new_end_tl: i64) {
        if self.send_command(Command::TrimSegmentEnd {
            segment_id,
            new_end_tl,
        }) {
            self.status = format!("segment {} trim-end to {}", segment_id, new_end_tl);
        }
    }

    fn flush_playhead_request(&mut self) {
        if self.playhead_request_in_flight {
            return;
        }

        let Some(t_tl) = self.pending_playhead_tl.take() else {
            return;
        };

        if let Some(sender) = &self.engine_tx {
            match sender.try_send(Command::SetPlayhead { t_tl }) {
                Ok(()) => {
                    self.playhead_request_in_flight = true;
                }
                Err(TrySendError::Full(_)) => {
                    self.pending_playhead_tl = Some(t_tl);
                    self.status = String::from("engine command queue is full");
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.status = String::from("engine command channel closed");
                    self.engine_tx = None;
                    self.playhead_request_in_flight = false;
                }
            }
        } else {
            self.pending_playhead_tl = Some(t_tl);
            self.status = String::from("engine is not ready");
        }
    }

    fn apply_engine_event(&mut self, event: Event) {
        match event {
            Event::ProjectChanged(snapshot) => {
                self.is_playing = false;
                self.project = Some(snapshot);
                self.playhead_tl = self.clamp_playhead(self.playhead_tl);
                self.preview_image = None;
                self.timeline_cache.clear();
                self.loaded_preview_ranges_tl.clear();
                self.pending_playhead_tl = None;
                self.latest_requested_playhead_tl = None;
                self.playhead_request_in_flight = false;
                self.idle_warm_target_tl = None;
                self.idle_warm_rounds = 0;
                self.last_split_tl = None;
                let pending_split_tl = self.pending_split_tl.take();
                let pending_cut_tl = self.pending_cut_tl.take();
                if let Some(split_tl) = pending_split_tl {
                    self.last_split_tl = Some(split_tl);
                    self.status = format!("split applied at {}", split_tl);
                } else if let Some(cut_tl) = pending_cut_tl {
                    self.status = format!("cut applied at {}", cut_tl);
                } else {
                    self.status = String::from("project loaded");
                }
            }
            Event::PlayheadChanged { t_tl } => {
                if !self.is_stale_playhead_event(t_tl) {
                    self.playhead_tl = self.clamp_playhead(t_tl);
                }
                self.playhead_request_in_flight = false;
                self.flush_playhead_request();
            }
            Event::PreviewFrameReady { t_tl, frame } => {
                if !self.is_stale_playhead_event(t_tl) {
                    self.playhead_tl = self.clamp_playhead(t_tl);
                    self.preview_image = preview::PreviewImage::from_frame(&frame);
                    self.record_loaded_preview_at(self.playhead_tl);
                    self.status = if self.preview_image.is_some() {
                        format!("preview ready at {}", self.playhead_tl)
                    } else {
                        String::from(
                            "preview frame dropped: unsupported format or invalid frame data",
                        )
                    };
                }
                self.playhead_request_in_flight = false;
                self.flush_playhead_request();
                if self.preview_image.is_some() && self.should_queue_idle_warm(t_tl) {
                    if self.idle_warm_target_tl != Some(t_tl) {
                        self.idle_warm_target_tl = Some(t_tl);
                        self.idle_warm_rounds = 0;
                    }
                    self.idle_warm_rounds = self.idle_warm_rounds.saturating_add(1);
                    self.record_idle_warm_loaded_hint(t_tl, self.idle_warm_rounds);
                    self.queue_playhead_for_idle_warm(t_tl);
                }
            }
            Event::ExportProgress { done, total } => {
                self.status = format!("exporting {done}/{total}");
            }
            Event::ExportFinished { path } => {
                self.status = format!("export finished: {}", path.display());
            }
            Event::Error(error) => {
                if let Some(split_tl) = self.pending_split_tl.take() {
                    self.last_split_tl = None;
                    if self.is_split_error(&error.kind) {
                        self.status = format!("split skipped at {}: {}", split_tl, error.message);
                    } else {
                        self.status = format!("split failed at {}: {}", split_tl, error.message);
                    }
                } else if let Some(cut_tl) = self.pending_cut_tl.take() {
                    if self.is_cut_error(&error.kind) {
                        self.status = format!("cut skipped at {}: {}", cut_tl, error.message);
                    } else {
                        self.status = format!("cut failed at {}: {}", cut_tl, error.message);
                    }
                } else {
                    self.status = format!("error: {}", error.message);
                }
                self.playhead_request_in_flight = false;
                self.flush_playhead_request();
            }
        }
    }

    fn is_stale_playhead_event(&self, event_t_tl: i64) -> bool {
        self.latest_requested_playhead_tl
            .is_some_and(|latest_t_tl| latest_t_tl != event_t_tl)
    }

    fn is_split_error(&self, kind: &EngineErrorKind) -> bool {
        matches!(
            kind,
            EngineErrorKind::SplitPointAtBoundary | EngineErrorKind::SegmentNotFound
        )
    }

    fn is_cut_error(&self, kind: &EngineErrorKind) -> bool {
        matches!(kind, EngineErrorKind::SegmentNotFound)
    }

    fn clamp_playhead(&self, t_tl: i64) -> i64 {
        match self.project.as_ref() {
            Some(snapshot) => {
                let max_tick = if snapshot.duration_tl <= 0 {
                    0
                } else {
                    snapshot.duration_tl - 1
                };
                t_tl.clamp(0, max_tick)
            }
            None => cmp::max(0, t_tl),
        }
    }

    fn record_loaded_preview_at(&mut self, t_tl: i64) {
        // Record only what is definitively loaded for this event.
        self.add_loaded_bucket_for_tick(t_tl);
    }

    fn add_loaded_bucket_for_tick(&mut self, t_tl: i64) {
        if t_tl < 0 {
            return;
        }

        let duration_tl = self.project.as_ref().map(|snapshot| snapshot.duration_tl);
        let end_tl = match duration_tl {
            Some(duration_tl) if duration_tl <= 0 => return,
            Some(duration_tl) => duration_tl,
            None => t_tl.saturating_add(self.preview_bucket_tl()),
        };
        let bucket_tl = self.preview_bucket_tl();
        let clamped_tl = match duration_tl {
            Some(duration_tl) => t_tl.clamp(0, duration_tl - 1),
            None => t_tl,
        };
        let bucket_start = clamped_tl.div_euclid(bucket_tl) * bucket_tl;
        let bucket_end = bucket_start.saturating_add(bucket_tl).min(end_tl);
        if bucket_end <= bucket_start {
            return;
        }
        self.add_loaded_preview_range(bucket_start, bucket_end);
    }

    fn add_loaded_preview_range(&mut self, start_tl: i64, end_tl: i64) {
        if start_tl >= end_tl {
            return;
        }

        let ranges = &mut self.loaded_preview_ranges_tl;
        let mut index = 0;
        while index < ranges.len() && ranges[index].1 < start_tl {
            index += 1;
        }

        if index < ranges.len() && ranges[index].0 <= start_tl && ranges[index].1 >= end_tl {
            return;
        }

        let mut merged_start = start_tl;
        let mut merged_end = end_tl;
        while index < ranges.len() && ranges[index].0 <= merged_end {
            merged_start = merged_start.min(ranges[index].0);
            merged_end = merged_end.max(ranges[index].1);
            ranges.remove(index);
        }
        ranges.insert(index, (merged_start, merged_end));
        self.timeline_cache.clear();
    }

    fn preview_bucket_tl(&self) -> i64 {
        self.project
            .as_ref()
            .map(|snapshot| snapshot.preview_bucket_tl)
            .unwrap_or(engine::DEFAULT_PREVIEW_CACHE_BUCKET_TL)
    }

    fn record_idle_warm_loaded_hint(&mut self, t_tl: i64, warm_round: u16) {
        let bucket_tl = self.preview_bucket_tl();
        let step = i64::from(warm_round.div_ceil(2));
        let direction = if warm_round % 2 == 1 { 1 } else { -1 };
        let offset = bucket_tl.saturating_mul(step).saturating_mul(direction);
        self.add_loaded_bucket_for_tick(t_tl.saturating_add(offset));
    }

    fn should_queue_idle_warm(&self, t_tl: i64) -> bool {
        !self.is_playing
            && self.project.is_some()
            && self.latest_requested_playhead_tl == Some(t_tl)
            && self.pending_playhead_tl.is_none()
            && !self.playhead_request_in_flight
            && self.idle_warm_target_tl.is_none_or(|target| target == t_tl)
            && self.idle_warm_rounds < IDLE_WARM_MAX_ROUNDS
    }

    /// Renders the UI tree.
    pub fn view(&self) -> Element<'_, Message> {
        let play_label = if self.is_playing { "Pause" } else { "Play" };
        let import_row = row![
            text_input("media path", &self.import_path).on_input(Message::ImportPathChanged),
            button("Import").on_press(Message::ImportPressed),
            button(play_label).on_press(Message::PlayPausePressed),
            button("Split").on_press(Message::SplitPressed),
            button("Cut").on_press(Message::CutPressed),
        ]
        .spacing(12);
        let export_row = row![
            text_input("export path", &self.export_path).on_input(Message::ExportPathChanged),
            button("Export").on_press(Message::ExportPressed),
        ]
        .spacing(12);

        let preview_widget = container(preview::view(self.preview_image.as_ref()))
            .width(Length::Fill)
            .height(Length::Fixed(240.0));

        let timeline_widget = timeline::view(
            self.project.as_ref(),
            self.playhead_tl,
            self.last_split_tl,
            &self.loaded_preview_ranges_tl,
            &self.timeline_cache,
            timeline::TimelineActions {
                on_scrub: Message::TimelineScrubbed,
                on_split: Message::TimelineSplitRequested,
                on_cut: Message::TimelineCutRequested,
                on_move: |segment_id, new_start_tl| Message::TimelineSegmentMoveRequested {
                    segment_id,
                    new_start_tl,
                },
                on_trim_start: |segment_id, new_start_tl| {
                    Message::TimelineSegmentTrimStartRequested {
                        segment_id,
                        new_start_tl,
                    }
                },
                on_trim_end: |segment_id, new_end_tl| Message::TimelineSegmentTrimEndRequested {
                    segment_id,
                    new_end_tl,
                },
            },
        );

        let controls = column![
            import_row,
            export_row,
            preview_widget,
            timeline_widget,
            text(format!("Playhead: {}", self.playhead_tl)),
            text(format!(
                "Playback: {}",
                if self.is_playing { "playing" } else { "paused" }
            )),
            text(format!(
                "Segments: {}",
                self.project
                    .as_ref()
                    .map(|snapshot| snapshot.segments.len())
                    .unwrap_or(0)
            )),
            text(format!("Status: {}", self.status)),
        ]
        .spacing(12)
        .padding(16);

        controls.into()
    }

    /// Subscribes to bridge events emitted by the engine worker thread.
    pub fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![engine_subscription().map(Message::Bridge)];
        if self.is_playing {
            let interval = Duration::from_micros(self.preview_bucket_tl().max(1) as u64);
            subscriptions.push(time::every(interval).map(Message::PlaybackTick));
        }

        Subscription::batch(subscriptions)
    }

    #[cfg(test)]
    fn from_sender_for_test(engine_tx: EngineCommandSender) -> Self {
        Self {
            engine_tx: Some(engine_tx),
            project: None,
            preview_image: None,
            import_path: String::new(),
            export_path: String::new(),
            playhead_tl: 0,
            pending_playhead_tl: None,
            latest_requested_playhead_tl: None,
            playhead_request_in_flight: false,
            idle_warm_target_tl: None,
            idle_warm_rounds: 0,
            loaded_preview_ranges_tl: Vec::new(),
            pending_split_tl: None,
            pending_cut_tl: None,
            last_split_tl: None,
            is_playing: false,
            timeline_cache: canvas::Cache::new(),
            status: String::from("idle"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::sync::mpsc::TryRecvError;
    use std::time::Duration;

    use engine::{Command, Event, ProjectSnapshot};

    use crate::bridge::BridgeEvent;

    use super::{AppState, Message};

    #[test]
    fn import_button_dispatches_import_command() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);

        let _ = app.update(Message::ImportPathChanged("demo.mp4".to_owned()));
        let _ = app.update(Message::ImportPressed);

        let command = command_rx.recv().expect("import command");
        assert_eq!(
            command,
            Command::Import {
                path: PathBuf::from("demo.mp4")
            }
        );
    }

    #[test]
    fn export_button_dispatches_export_command() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);

        let _ = app.update(Message::ExportPathChanged("out.mp4".to_owned()));
        let _ = app.update(Message::ExportPressed);

        let command = command_rx.recv().expect("export command");
        assert_eq!(
            command,
            Command::Export {
                path: PathBuf::from("out.mp4"),
                settings: engine::ExportSettings::default(),
            }
        );
    }

    #[test]
    fn export_button_rejects_empty_path() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);

        let _ = app.update(Message::ExportPathChanged("   ".to_owned()));
        let _ = app.update(Message::ExportPressed);

        assert_eq!(app.status, "export path is empty");
        assert!(matches!(command_rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn timeline_scrub_dispatches_set_playhead_command() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);

        let _ = app.update(Message::TimelineScrubbed(42));

        let command = command_rx.recv().expect("set playhead command");
        assert_eq!(command, Command::SetPlayhead { t_tl: 42 });
    }

    #[test]
    fn play_button_starts_playback_and_dispatches_set_playhead_command() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(40));
        let _ = command_rx.recv().expect("set playhead command");
        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PlayheadChanged { t_tl: 40 },
        )));

        let _ = app.update(Message::PlayPausePressed);

        let command = command_rx.recv().expect("play warm set playhead command");
        assert_eq!(command, Command::SetPlayhead { t_tl: 40 });
        assert!(app.is_playing);
    }

    #[test]
    fn playback_tick_advances_playhead_by_preview_bucket_and_dispatches_command() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 200_000,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::PlayPausePressed);
        let first = command_rx.recv().expect("play warm set playhead command");
        assert_eq!(first, Command::SetPlayhead { t_tl: 0 });
        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PlayheadChanged { t_tl: 0 },
        )));

        let _ = app.update(Message::PlaybackTick(std::time::Instant::now()));

        let second = command_rx
            .recv()
            .expect("playback step set playhead command");
        assert_eq!(second, Command::SetPlayhead { t_tl: 33_333 });
    }

    #[test]
    fn playback_tick_at_timeline_end_stops_playback_without_dispatching_command() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 50,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(49));
        let _ = command_rx.recv().expect("set playhead command");
        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PlayheadChanged { t_tl: 49 },
        )));

        let _ = app.update(Message::PlayPausePressed);
        let _ = command_rx.recv().expect("play warm set playhead command");
        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PlayheadChanged { t_tl: 49 },
        )));

        let _ = app.update(Message::PlaybackTick(std::time::Instant::now()));

        assert!(!app.is_playing);
        assert!(matches!(command_rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn timeline_scrub_clamps_to_last_timeline_tick() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(100));

        let command = command_rx.recv().expect("set playhead command");
        assert_eq!(command, Command::SetPlayhead { t_tl: 99 });
    }

    #[test]
    fn split_button_dispatches_split_at_current_playhead() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::TimelineScrubbed(250_000));
        let _ = command_rx.recv().expect("set playhead command");

        let _ = app.update(Message::SplitPressed);

        let command = command_rx.recv().expect("split command");
        assert_eq!(command, Command::Split { at_tl: 250_000 });
    }

    #[test]
    fn cut_button_dispatches_cut_at_current_playhead() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::TimelineScrubbed(250_000));
        let _ = command_rx.recv().expect("set playhead command");

        let _ = app.update(Message::CutPressed);

        let command = command_rx.recv().expect("cut command");
        assert_eq!(command, Command::Cut { at_tl: 250_000 });
    }

    #[test]
    fn split_button_queues_playhead_refresh_after_in_flight_preview() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(40));
        let first = command_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("first set playhead command");
        assert_eq!(first, Command::SetPlayhead { t_tl: 40 });

        let _ = app.update(Message::SplitPressed);
        let split = command_rx.recv().expect("split command");
        assert_eq!(split, Command::Split { at_tl: 40 });
        assert!(matches!(command_rx.try_recv(), Err(TryRecvError::Empty)));

        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PreviewFrameReady {
                t_tl: 40,
                frame: engine::PreviewFrame {
                    width: 1,
                    height: 1,
                    format: engine::PreviewPixelFormat::Rgba8,
                    bytes: std::sync::Arc::from(vec![0_u8; 4]),
                },
            },
        )));

        let refreshed = command_rx.recv().expect("refreshed set playhead command");
        assert_eq!(refreshed, Command::SetPlayhead { t_tl: 40 });
    }

    #[test]
    fn split_requests_are_not_sent_while_another_split_is_pending() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(40));
        let _ = command_rx.recv().expect("first set playhead command");

        let _ = app.update(Message::SplitPressed);
        let first_split = command_rx.recv().expect("first split command");
        assert_eq!(first_split, Command::Split { at_tl: 40 });

        let _ = app.update(Message::SplitPressed);
        assert_eq!(app.status, "split request is already pending");
        assert!(matches!(command_rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn bridge_event_applies_playhead_changed_event() {
        let (command_tx, _command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);

        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PlayheadChanged { t_tl: 1234 },
        )));

        assert_eq!(app.playhead_tl, 1234);
    }

    #[test]
    fn timeline_scrub_coalesces_pending_playhead_updates() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(10));
        let _ = app.update(Message::TimelineScrubbed(20));
        let _ = app.update(Message::TimelineScrubbed(30));

        let first = command_rx.recv().expect("first set playhead command");
        assert_eq!(first, Command::SetPlayhead { t_tl: 10 });
        assert!(matches!(command_rx.try_recv(), Err(TryRecvError::Empty)));

        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PreviewFrameReady {
                t_tl: 10,
                frame: engine::PreviewFrame {
                    width: 1,
                    height: 1,
                    format: engine::PreviewPixelFormat::Rgba8,
                    bytes: std::sync::Arc::from(vec![0_u8; 4]),
                },
            },
        )));
        assert_eq!(app.playhead_tl, 30);

        let second = command_rx.recv().expect("second set playhead command");
        assert_eq!(second, Command::SetPlayhead { t_tl: 30 });
    }

    #[test]
    fn stale_playhead_changed_event_does_not_override_latest_scrubbed_playhead() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(10));
        let _ = command_rx.recv().expect("first set playhead command");
        let _ = app.update(Message::TimelineScrubbed(80));

        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PlayheadChanged { t_tl: 10 },
        )));

        assert_eq!(app.playhead_tl, 80);

        let second = command_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("second set playhead command");
        assert_eq!(second, Command::SetPlayhead { t_tl: 80 });
    }

    #[test]
    fn stale_preview_frame_ready_event_does_not_override_latest_scrubbed_playhead() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(10));
        let first = command_rx.recv().expect("first set playhead command");
        assert_eq!(first, Command::SetPlayhead { t_tl: 10 });

        let _ = app.update(Message::TimelineScrubbed(80));
        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PlayheadChanged { t_tl: 10 },
        )));
        let second = command_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("second set playhead command");
        assert_eq!(second, Command::SetPlayhead { t_tl: 80 });

        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PreviewFrameReady {
                t_tl: 10,
                frame: engine::PreviewFrame {
                    width: 1,
                    height: 1,
                    format: engine::PreviewPixelFormat::Rgba8,
                    bytes: std::sync::Arc::from(vec![0_u8; 4]),
                },
            },
        )));

        assert_eq!(app.playhead_tl, 80);
        assert!(app.preview_image.is_none());
    }

    #[test]
    fn preview_frame_ready_updates_loaded_ranges_and_project_changed_clears_them() {
        let (command_tx, _command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 200_000,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PreviewFrameReady {
                t_tl: 40_000,
                frame: engine::PreviewFrame {
                    width: 2,
                    height: 1,
                    format: engine::PreviewPixelFormat::Rgba8,
                    bytes: std::sync::Arc::from(vec![0_u8; 8]),
                },
            },
        )));

        assert!(range_contains_tick(&app.loaded_preview_ranges_tl, 40_000));
        assert!(!range_contains_tick(&app.loaded_preview_ranges_tl, 73_333));

        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 200_000,
                preview_bucket_tl: 33_333,
            },
        ))));
        assert!(app.loaded_preview_ranges_tl.is_empty());
    }

    #[test]
    fn preview_frame_ready_when_idle_queues_additional_playhead_for_progressive_warm_prefetch() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(40));
        let first = command_rx.recv().expect("first set playhead command");
        assert_eq!(first, Command::SetPlayhead { t_tl: 40 });

        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PreviewFrameReady {
                t_tl: 40,
                frame: engine::PreviewFrame {
                    width: 1,
                    height: 1,
                    format: engine::PreviewPixelFormat::Rgba8,
                    bytes: std::sync::Arc::from(vec![0_u8; 4]),
                },
            },
        )));
        assert!(app.preview_image.is_some());
        assert_eq!(app.latest_requested_playhead_tl, Some(40));
        assert!(app.playhead_request_in_flight);
        assert!(app.pending_playhead_tl.is_none());
        assert_eq!(app.idle_warm_target_tl, Some(40));

        let second = command_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("idle warm set playhead command");
        assert_eq!(second, Command::SetPlayhead { t_tl: 40 });

        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PreviewFrameReady {
                t_tl: 40,
                frame: engine::PreviewFrame {
                    width: 1,
                    height: 1,
                    format: engine::PreviewPixelFormat::Rgba8,
                    bytes: std::sync::Arc::from(vec![0_u8; 4]),
                },
            },
        )));
        let third = command_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("second idle warm set playhead command");
        assert_eq!(third, Command::SetPlayhead { t_tl: 40 });
    }

    #[test]
    fn scrub_after_idle_warm_in_flight_still_updates_preview() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 200,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(40));
        let first = command_rx.recv().expect("first set playhead command");
        assert_eq!(first, Command::SetPlayhead { t_tl: 40 });

        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PreviewFrameReady {
                t_tl: 40,
                frame: engine::PreviewFrame {
                    width: 1,
                    height: 1,
                    format: engine::PreviewPixelFormat::Rgba8,
                    bytes: std::sync::Arc::from(vec![0_u8; 4]),
                },
            },
        )));
        let warm = command_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("idle warm set playhead command");
        assert_eq!(warm, Command::SetPlayhead { t_tl: 40 });

        let _ = app.update(Message::TimelineScrubbed(80));
        assert!(matches!(command_rx.try_recv(), Err(TryRecvError::Empty)));

        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PlayheadChanged { t_tl: 40 },
        )));
        let second = command_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("second set playhead command");
        assert_eq!(second, Command::SetPlayhead { t_tl: 80 });

        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PreviewFrameReady {
                t_tl: 40,
                frame: engine::PreviewFrame {
                    width: 1,
                    height: 1,
                    format: engine::PreviewPixelFormat::Rgba8,
                    bytes: std::sync::Arc::from(vec![0_u8; 4]),
                },
            },
        )));
        assert_eq!(app.playhead_tl, 80);

        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PreviewFrameReady {
                t_tl: 80,
                frame: engine::PreviewFrame {
                    width: 1,
                    height: 1,
                    format: engine::PreviewPixelFormat::Rgba8,
                    bytes: std::sync::Arc::from(vec![1_u8; 4]),
                },
            },
        )));

        assert_eq!(app.playhead_tl, 80);
        assert!(app.preview_image.is_some());
    }

    #[test]
    fn bridge_preview_frame_ready_keeps_latest_preview_image() {
        let (command_tx, _command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);

        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PreviewFrameReady {
                t_tl: 12,
                frame: engine::PreviewFrame {
                    width: 2,
                    height: 1,
                    format: engine::PreviewPixelFormat::Rgba8,
                    bytes: std::sync::Arc::from(vec![0_u8; 8]),
                },
            },
        )));

        assert!(app.preview_image.is_some());
    }

    #[test]
    fn bridge_preview_frame_ready_with_invalid_rgba_data_sets_generic_drop_status() {
        let (command_tx, _command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);

        let _ = app.update(Message::Bridge(BridgeEvent::Event(
            Event::PreviewFrameReady {
                t_tl: 12,
                frame: engine::PreviewFrame {
                    width: 2,
                    height: 1,
                    format: engine::PreviewPixelFormat::Rgba8,
                    bytes: std::sync::Arc::from(vec![0_u8; 3]),
                },
            },
        )));

        assert_eq!(
            app.status,
            "preview frame dropped: unsupported format or invalid frame data"
        );
    }

    #[test]
    fn timeline_split_requested_dispatches_split_command() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineSplitRequested(100));

        let split = command_rx.recv().expect("split command");
        assert_eq!(split, Command::Split { at_tl: 99 });

        let set_playhead = command_rx.recv().expect("set playhead command");
        assert_eq!(set_playhead, Command::SetPlayhead { t_tl: 99 });
    }

    #[test]
    fn timeline_cut_requested_dispatches_cut_command() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineCutRequested(100));

        let cut = command_rx.recv().expect("cut command");
        assert_eq!(cut, Command::Cut { at_tl: 99 });

        let set_playhead = command_rx.recv().expect("set playhead command");
        assert_eq!(set_playhead, Command::SetPlayhead { t_tl: 99 });
    }

    #[test]
    fn timeline_segment_move_requested_dispatches_move_segment_command() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);

        let _ = app.update(Message::TimelineSegmentMoveRequested {
            segment_id: 7,
            new_start_tl: 345_000,
        });

        let command = command_rx.recv().expect("move segment command");
        assert_eq!(
            command,
            Command::MoveSegment {
                segment_id: 7,
                new_start_tl: 345_000,
            }
        );
    }

    #[test]
    fn timeline_segment_trim_start_requested_dispatches_trim_start_command() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);

        let _ = app.update(Message::TimelineSegmentTrimStartRequested {
            segment_id: 7,
            new_start_tl: 123_000,
        });

        let command = command_rx.recv().expect("trim start command");
        assert_eq!(
            command,
            Command::TrimSegmentStart {
                segment_id: 7,
                new_start_tl: 123_000,
            }
        );
    }

    #[test]
    fn timeline_segment_trim_end_requested_dispatches_trim_end_command() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);

        let _ = app.update(Message::TimelineSegmentTrimEndRequested {
            segment_id: 7,
            new_end_tl: 456_000,
        });

        let command = command_rx.recv().expect("trim end command");
        assert_eq!(
            command,
            Command::TrimSegmentEnd {
                segment_id: 7,
                new_end_tl: 456_000,
            }
        );
    }

    #[test]
    fn project_changed_resets_in_flight_state_and_allows_new_scrub_dispatch() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);

        let _ = app.update(Message::TimelineScrubbed(0));
        let first = command_rx.recv().expect("first set playhead command");
        assert_eq!(first, Command::SetPlayhead { t_tl: 0 });

        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));
        let _ = app.update(Message::TimelineScrubbed(60));

        let second = command_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("second set playhead command");
        assert_eq!(second, Command::SetPlayhead { t_tl: 60 });
    }

    #[test]
    fn split_success_updates_status_and_keeps_split_feedback_tick() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(30));
        let _ = command_rx.recv().expect("set playhead command");
        let _ = app.update(Message::SplitPressed);
        let _ = command_rx.recv().expect("split command");
        assert_eq!(app.pending_split_tl, Some(30));

        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        assert_eq!(app.status, "split applied at 30");
        assert_eq!(app.pending_split_tl, None);
        assert_eq!(app.last_split_tl, Some(30));
    }

    #[test]
    fn split_failure_updates_status_with_context_and_clears_pending_feedback() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(99));
        let _ = command_rx.recv().expect("set playhead command");
        let _ = app.update(Message::SplitPressed);
        let _ = command_rx.recv().expect("split command");
        assert_eq!(app.pending_split_tl, Some(99));

        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::Error(
            engine::EngineErrorEvent {
                kind: engine::EngineErrorKind::SplitPointAtBoundary,
                message: "cannot split at segment boundary: 99".to_owned(),
            },
        ))));

        assert_eq!(
            app.status,
            "split skipped at 99: cannot split at segment boundary: 99"
        );
        assert_eq!(app.pending_split_tl, None);
        assert_eq!(app.last_split_tl, None);
    }

    #[test]
    fn split_failure_clears_previous_split_marker() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(30));
        let _ = command_rx.recv().expect("set playhead command");
        let _ = app.update(Message::SplitPressed);
        let _ = command_rx.recv().expect("split command");
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));
        assert_eq!(app.last_split_tl, Some(30));

        let _ = app.update(Message::TimelineScrubbed(99));
        let _ = command_rx.recv().expect("set playhead command");
        let _ = app.update(Message::SplitPressed);
        let _ = command_rx.recv().expect("split command");
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::Error(
            engine::EngineErrorEvent {
                kind: engine::EngineErrorKind::SplitPointAtBoundary,
                message: "cannot split at segment boundary: 99".to_owned(),
            },
        ))));

        assert_eq!(app.last_split_tl, None);
    }

    #[test]
    fn non_split_error_clears_pending_split_feedback() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(42));
        let _ = command_rx.recv().expect("set playhead command");
        let _ = app.update(Message::SplitPressed);
        let _ = command_rx.recv().expect("split command");

        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::Error(
            engine::EngineErrorEvent {
                kind: engine::EngineErrorKind::Other,
                message: "media backend error: decode failed".to_owned(),
            },
        ))));

        assert_eq!(
            app.status,
            "split failed at 42: media backend error: decode failed"
        );
        assert_eq!(app.pending_split_tl, None);
        assert_eq!(app.last_split_tl, None);

        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));
        assert_eq!(app.pending_split_tl, None);
        assert_eq!(app.last_split_tl, None);
    }

    #[test]
    fn split_like_error_message_still_clears_pending_when_kind_is_other() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(50));
        let _ = command_rx.recv().expect("set playhead command");
        let _ = app.update(Message::SplitPressed);
        let _ = command_rx.recv().expect("split command");

        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::Error(
            engine::EngineErrorEvent {
                kind: engine::EngineErrorKind::Other,
                message: "cannot split at segment boundary: 50".to_owned(),
            },
        ))));

        assert_eq!(
            app.status,
            "split failed at 50: cannot split at segment boundary: 50"
        );
        assert_eq!(app.pending_split_tl, None);
        assert_eq!(app.last_split_tl, None);
    }

    #[test]
    fn non_cut_error_clears_pending_cut_feedback() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(40));
        let _ = command_rx.recv().expect("set playhead command");
        let _ = app.update(Message::CutPressed);
        let _ = command_rx.recv().expect("cut command");
        assert_eq!(app.pending_cut_tl, Some(40));

        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::Error(
            engine::EngineErrorEvent {
                kind: engine::EngineErrorKind::Other,
                message: "mux failed".to_owned(),
            },
        ))));

        assert_eq!(app.status, "cut failed at 40: mux failed");
        assert_eq!(app.pending_cut_tl, None);
    }

    #[test]
    fn bridge_disconnected_clears_split_feedback() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(30));
        let _ = command_rx.recv().expect("set playhead command");
        let _ = app.update(Message::SplitPressed);
        let _ = command_rx.recv().expect("split command");
        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::ProjectChanged(
            ProjectSnapshot {
                assets: vec![],
                segments: vec![],
                duration_tl: 100,
                preview_bucket_tl: 33_333,
            },
        ))));
        assert_eq!(app.last_split_tl, Some(30));

        let _ = app.update(Message::Bridge(BridgeEvent::Disconnected));

        assert_eq!(app.last_split_tl, None);
    }

    fn range_contains_tick(ranges: &[(i64, i64)], tick: i64) -> bool {
        ranges
            .iter()
            .any(|(start, end)| *start <= tick && tick < *end)
    }
}
