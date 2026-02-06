use std::sync::mpsc;
use std::thread;

use engine::{Command, Engine, Event, MediaBackend};

/// Sender used by the UI thread to dispatch commands to the engine thread.
pub type EngineCommandSender = mpsc::Sender<Command>;

/// Receiver used by the UI thread to read events emitted by the engine thread.
pub type EngineEventReceiver = mpsc::Receiver<Event>;

/// Spawns the production bridge that wires a FFmpeg-backed engine.
pub fn spawn_ffmpeg_bridge() -> (EngineCommandSender, EngineEventReceiver) {
    spawn_engine_bridge(Engine::with_ffmpeg())
}

/// Spawns a bridge around any engine backend.
pub fn spawn_engine_bridge<M>(mut engine: Engine<M>) -> (EngineCommandSender, EngineEventReceiver)
where
    M: MediaBackend + Send + 'static,
{
    let (command_tx, command_rx) = mpsc::channel::<Command>();
    let (event_tx, event_rx) = mpsc::channel::<Event>();

    thread::spawn(move || {
        while let Ok(command) = command_rx.recv() {
            match engine.handle_command(command) {
                Ok(events) => {
                    for event in events {
                        if event_tx.send(event).is_err() {
                            return;
                        }
                    }
                }
                Err(error) => {
                    if event_tx
                        .send(Event::Error(engine::api::EngineErrorEvent {
                            message: error.to_string(),
                        }))
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
    });

    (command_tx, event_rx)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use engine::Rational;
    use engine::preview::{PreviewFrame, PreviewPixelFormat, ProbedMedia, ProbedVideoStream};

    use super::{Command, Engine, Event, MediaBackend, spawn_engine_bridge};

    #[test]
    fn bridge_forwards_engine_events_for_import_command() {
        let (command_tx, event_rx) = spawn_engine_bridge(Engine::new(MockBackend));

        command_tx
            .send(Command::Import {
                path: PathBuf::from("demo.mp4"),
            })
            .expect("send import command");

        let first = event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first event");
        let second = event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("second event");

        assert!(matches!(first, Event::ProjectChanged(_)));
        assert_eq!(second, Event::PlayheadChanged { t_tl: 0 });
    }

    #[test]
    fn bridge_emits_error_event_when_command_fails() {
        let (command_tx, event_rx) = spawn_engine_bridge(Engine::new(MockBackend));

        command_tx
            .send(Command::SetPlayhead { t_tl: 10 })
            .expect("send set playhead command");

        let event = event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("error event");

        let Event::Error(error) = event else {
            panic!("expected Event::Error");
        };
        assert!(error.message.contains("project is not loaded"));
    }

    #[derive(Debug, Clone, Copy)]
    struct MockBackend;

    impl MediaBackend for MockBackend {
        fn probe(&self, path: &Path) -> engine::Result<ProbedMedia> {
            Ok(ProbedMedia {
                path: path.to_path_buf(),
                duration_tl: 1_000_000,
                video: Some(ProbedVideoStream {
                    time_base: Rational::new(1, 90_000).expect("valid rational"),
                    src_in: 0,
                    src_out: 90_000,
                    width: 160,
                    height: 90,
                }),
                audio: None,
            })
        }

        fn decode_preview_frame(
            &self,
            _path: &Path,
            _at_seconds: f64,
        ) -> engine::Result<PreviewFrame> {
            Ok(PreviewFrame {
                width: 160,
                height: 90,
                format: PreviewPixelFormat::Rgba8,
                bytes: Arc::from(vec![0; 160 * 90 * 4]),
            })
        }

        fn export_video(&self, _plan: &engine::export::ExportVideoPlan) -> engine::Result<()> {
            Ok(())
        }
    }
}
