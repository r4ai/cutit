use std::path::PathBuf;
use std::{cmp, sync::mpsc::TrySendError};

use engine::{Command, Event, ProjectSnapshot};
use iced::widget::canvas;
use iced::widget::{button, column, container, row, text, text_input};
use iced::{Element, Length, Subscription, Task};

use crate::bridge::{BridgeEvent, EngineCommandSender, engine_subscription};
use crate::widgets::{preview, timeline};

/// UI messages handled by the iced app update loop.
#[derive(Debug, Clone)]
pub enum Message {
    ImportPathChanged(String),
    ImportPressed,
    SplitPressed,
    TimelineScrubbed(i64),
    TimelineSplitRequested(i64),
    Bridge(BridgeEvent),
}

/// Root UI state for Step 6 bootstrap.
pub struct AppState {
    engine_tx: Option<EngineCommandSender>,
    project: Option<ProjectSnapshot>,
    preview_image: Option<preview::PreviewImage>,
    import_path: String,
    playhead_tl: i64,
    pending_playhead_tl: Option<i64>,
    playhead_request_in_flight: bool,
    pending_split_tl: Option<i64>,
    last_split_tl: Option<i64>,
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
                playhead_tl: 0,
                pending_playhead_tl: None,
                playhead_request_in_flight: false,
                pending_split_tl: None,
                last_split_tl: None,
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
                    self.last_split_tl = None;
                    self.status = format!("importing {}", path);
                }
            }
            Message::SplitPressed => {
                self.request_split(self.playhead_tl);
            }
            Message::TimelineScrubbed(t_tl) => {
                let clamped = self.clamp_playhead(t_tl);
                self.playhead_tl = clamped;
                self.queue_playhead(clamped);
            }
            Message::TimelineSplitRequested(at_tl) => {
                let clamped = self.clamp_playhead(at_tl);
                self.playhead_tl = clamped;
                self.request_split(clamped);
                self.queue_playhead(clamped);
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
                self.playhead_request_in_flight = false;
                self.pending_split_tl = None;
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

    fn queue_playhead(&mut self, t_tl: i64) {
        self.pending_playhead_tl = Some(t_tl);
        self.flush_playhead_request();
    }

    fn request_split(&mut self, at_tl: i64) {
        let clamped = self.clamp_playhead(at_tl);
        self.playhead_tl = clamped;
        if self.send_command(Command::Split { at_tl: clamped }) {
            self.pending_split_tl = Some(clamped);
            self.status = format!("split requested at {}", clamped);
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
                self.project = Some(snapshot);
                self.playhead_tl = self.clamp_playhead(self.playhead_tl);
                self.preview_image = None;
                self.timeline_cache.clear();
                self.pending_playhead_tl = None;
                self.playhead_request_in_flight = false;
                self.last_split_tl = None;
                if let Some(split_tl) = self.pending_split_tl.take() {
                    self.last_split_tl = Some(split_tl);
                    self.status = format!("split applied at {}", split_tl);
                } else {
                    self.status = String::from("project loaded");
                }
            }
            Event::PlayheadChanged { t_tl } => {
                self.playhead_tl = self.clamp_playhead(t_tl);
            }
            Event::PreviewFrameReady { t_tl, frame } => {
                self.playhead_tl = self.clamp_playhead(t_tl);
                self.preview_image = preview::PreviewImage::from_frame(&frame);
                self.status = if self.preview_image.is_some() {
                    format!("preview ready at {}", self.playhead_tl)
                } else {
                    String::from("preview frame dropped: unsupported format or invalid frame data")
                };
                self.playhead_request_in_flight = false;
                self.flush_playhead_request();
            }
            Event::ExportProgress { done, total } => {
                self.status = format!("exporting {done}/{total}");
            }
            Event::ExportFinished { path } => {
                self.status = format!("export finished: {}", path.display());
            }
            Event::Error(error) => {
                if let Some(split_tl) = self.pending_split_tl.take() {
                    self.status = format!("split skipped at {}: {}", split_tl, error.message);
                } else {
                    self.status = format!("error: {}", error.message);
                }
                self.playhead_request_in_flight = false;
                self.flush_playhead_request();
            }
        }
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

    /// Renders the UI tree.
    pub fn view(&self) -> Element<'_, Message> {
        let import_row = row![
            text_input("media path", &self.import_path).on_input(Message::ImportPathChanged),
            button("Import").on_press(Message::ImportPressed),
            button("Split").on_press(Message::SplitPressed),
        ]
        .spacing(12);

        let preview_widget = container(preview::view(self.preview_image.as_ref()))
            .width(Length::Fill)
            .height(Length::Fixed(240.0));

        let timeline_widget = timeline::view(
            self.project.as_ref(),
            self.playhead_tl,
            self.last_split_tl,
            &self.timeline_cache,
            Message::TimelineScrubbed,
            Message::TimelineSplitRequested,
        );

        let controls = column![
            import_row,
            preview_widget,
            timeline_widget,
            text(format!("Playhead: {}", self.playhead_tl)),
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
        engine_subscription().map(Message::Bridge)
    }

    #[cfg(test)]
    fn from_sender_for_test(engine_tx: EngineCommandSender) -> Self {
        Self {
            engine_tx: Some(engine_tx),
            project: None,
            preview_image: None,
            import_path: String::new(),
            playhead_tl: 0,
            pending_playhead_tl: None,
            playhead_request_in_flight: false,
            pending_split_tl: None,
            last_split_tl: None,
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
    fn timeline_scrub_dispatches_set_playhead_command() {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let mut app = AppState::from_sender_for_test(command_tx);

        let _ = app.update(Message::TimelineScrubbed(42));

        let command = command_rx.recv().expect("set playhead command");
        assert_eq!(command, Command::SetPlayhead { t_tl: 42 });
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

        let second = command_rx.recv().expect("second set playhead command");
        assert_eq!(second, Command::SetPlayhead { t_tl: 30 });
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
            },
        ))));

        let _ = app.update(Message::TimelineSplitRequested(100));

        let split = command_rx.recv().expect("split command");
        assert_eq!(split, Command::Split { at_tl: 99 });

        let set_playhead = command_rx.recv().expect("set playhead command");
        assert_eq!(set_playhead, Command::SetPlayhead { t_tl: 99 });
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
            },
        ))));

        let _ = app.update(Message::TimelineScrubbed(99));
        let _ = command_rx.recv().expect("set playhead command");
        let _ = app.update(Message::SplitPressed);
        let _ = command_rx.recv().expect("split command");
        assert_eq!(app.pending_split_tl, Some(99));

        let _ = app.update(Message::Bridge(BridgeEvent::Event(Event::Error(
            engine::EngineErrorEvent {
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
}
