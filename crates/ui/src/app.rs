use std::path::PathBuf;
use std::time::Duration;
use std::{cmp, sync::mpsc::TryRecvError};

use engine::{Command, Event, ProjectSnapshot};
use iced::widget::{button, column, row, slider, text, text_input};
use iced::{Element, Subscription, Task, time};

use crate::bridge::{EngineCommandSender, EngineEventReceiver, spawn_ffmpeg_bridge};

/// UI messages handled by the iced app update loop.
#[derive(Debug, Clone)]
pub enum Message {
    ImportPathChanged(String),
    ImportPressed,
    SplitPressed,
    TimelineScrubbed(f64),
    PollEngine,
}

/// Root UI state for Step 6 bootstrap.
pub struct AppState {
    engine_tx: Option<EngineCommandSender>,
    engine_rx: Option<EngineEventReceiver>,
    project: Option<ProjectSnapshot>,
    import_path: String,
    playhead_tl: i64,
    status: String,
}

impl AppState {
    /// Boots the app and initializes the engine bridge.
    pub fn boot() -> (Self, Task<Message>) {
        let (engine_tx, engine_rx) = spawn_ffmpeg_bridge();
        (
            Self {
                engine_tx: Some(engine_tx),
                engine_rx: Some(engine_rx),
                project: None,
                import_path: String::new(),
                playhead_tl: 0,
                status: String::from("ready"),
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
                } else {
                    self.send_command(Command::Import {
                        path: PathBuf::from(&path),
                    });
                    self.status = format!("importing {}", path);
                }
            }
            Message::SplitPressed => {
                self.send_command(Command::Split {
                    at_tl: self.playhead_tl,
                });
                self.status = format!("split requested at {}", self.playhead_tl);
            }
            Message::TimelineScrubbed(t_tl) => {
                let clamped = self.clamp_playhead(t_tl.round() as i64);
                self.playhead_tl = clamped;
                self.send_command(Command::SetPlayhead { t_tl: clamped });
            }
            Message::PollEngine => self.poll_engine_events(),
        }

        Task::none()
    }

    fn send_command(&mut self, command: Command) {
        if let Some(sender) = &self.engine_tx {
            if sender.send(command).is_err() {
                self.status = String::from("engine command channel closed");
                self.engine_tx = None;
                self.engine_rx = None;
            }
        } else {
            self.status = String::from("engine is not ready");
        }
    }

    fn poll_engine_events(&mut self) {
        loop {
            let event = match self.engine_rx.as_ref() {
                Some(receiver) => match receiver.try_recv() {
                    Ok(event) => event,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.status = String::from("engine event channel closed");
                        self.engine_tx = None;
                        self.engine_rx = None;
                        break;
                    }
                },
                None => break,
            };

            self.apply_engine_event(event);
        }
    }

    fn apply_engine_event(&mut self, event: Event) {
        match event {
            Event::ProjectChanged(snapshot) => {
                self.project = Some(snapshot);
                self.playhead_tl = self.clamp_playhead(self.playhead_tl);
                self.status = String::from("project loaded");
            }
            Event::PlayheadChanged { t_tl } => {
                self.playhead_tl = self.clamp_playhead(t_tl);
            }
            Event::PreviewFrameReady { t_tl, .. } => {
                self.playhead_tl = self.clamp_playhead(t_tl);
                self.status = format!("preview ready at {}", self.playhead_tl);
            }
            Event::ExportProgress { done, total } => {
                self.status = format!("exporting {done}/{total}");
            }
            Event::ExportFinished { path } => {
                self.status = format!("export finished: {}", path.display());
            }
            Event::Error(error) => {
                self.status = format!("error: {}", error.message);
            }
        }
    }

    fn clamp_playhead(&self, t_tl: i64) -> i64 {
        match self.project.as_ref() {
            Some(snapshot) => cmp::max(0, cmp::min(t_tl, snapshot.duration_tl)),
            None => cmp::max(0, t_tl),
        }
    }

    /// Renders the UI tree.
    pub fn view(&self) -> Element<'_, Message> {
        let max_playhead = self
            .project
            .as_ref()
            .map(|snapshot| snapshot.duration_tl)
            .unwrap_or(0);

        let import_row = row![
            text_input("media path", &self.import_path).on_input(Message::ImportPathChanged),
            button("Import").on_press(Message::ImportPressed),
            button("Split").on_press(Message::SplitPressed),
        ]
        .spacing(12);

        let controls = column![
            import_row,
            slider(
                0.0..=(max_playhead as f64),
                self.playhead_tl as f64,
                Message::TimelineScrubbed
            )
            .step(1.0),
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

    /// Polls engine events at a fixed interval.
    pub fn subscription(&self) -> Subscription<Message> {
        time::every(Duration::from_millis(16)).map(|_| Message::PollEngine)
    }

    #[cfg(test)]
    fn from_bridge_for_test(
        engine_tx: EngineCommandSender,
        engine_rx: EngineEventReceiver,
    ) -> Self {
        Self {
            engine_tx: Some(engine_tx),
            engine_rx: Some(engine_rx),
            project: None,
            import_path: String::new(),
            playhead_tl: 0,
            status: String::from("idle"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::mpsc;

    use engine::{Command, Event};

    use super::{AppState, Message};

    #[test]
    fn import_button_dispatches_import_command() {
        let (command_tx, command_rx) = mpsc::channel();
        let (_event_tx, event_rx) = mpsc::channel();
        let mut app = AppState::from_bridge_for_test(command_tx, event_rx);

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
        let (command_tx, command_rx) = mpsc::channel();
        let (_event_tx, event_rx) = mpsc::channel();
        let mut app = AppState::from_bridge_for_test(command_tx, event_rx);

        let _ = app.update(Message::TimelineScrubbed(42.0));

        let command = command_rx.recv().expect("set playhead command");
        assert_eq!(command, Command::SetPlayhead { t_tl: 42 });
    }

    #[test]
    fn split_button_dispatches_split_at_current_playhead() {
        let (command_tx, command_rx) = mpsc::channel();
        let (_event_tx, event_rx) = mpsc::channel();
        let mut app = AppState::from_bridge_for_test(command_tx, event_rx);
        let _ = app.update(Message::TimelineScrubbed(250_000.0));
        let _ = command_rx.recv().expect("set playhead command");

        let _ = app.update(Message::SplitPressed);

        let command = command_rx.recv().expect("split command");
        assert_eq!(command, Command::Split { at_tl: 250_000 });
    }

    #[test]
    fn polling_applies_playhead_changed_event() {
        let (command_tx, _command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let mut app = AppState::from_bridge_for_test(command_tx, event_rx);

        event_tx
            .send(Event::PlayheadChanged { t_tl: 1234 })
            .expect("send event");

        let _ = app.update(Message::PollEngine);

        assert_eq!(app.playhead_tl, 1234);
    }
}
