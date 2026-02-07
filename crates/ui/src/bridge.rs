use std::sync::mpsc;
use std::thread;

use engine::{Command, Engine, EngineErrorEvent, Event, MediaBackend};
use iced::futures::{SinkExt, StreamExt, channel::mpsc as futures_mpsc, executor};
use iced::{Subscription, stream};

const COMMAND_CHANNEL_CAPACITY: usize = 32;
const EVENT_CHANNEL_CAPACITY: usize = 8;
const SUBSCRIPTION_CHANNEL_CAPACITY: usize = 32;

/// Sender used by the UI thread to dispatch commands to the engine thread.
pub type EngineCommandSender = mpsc::SyncSender<Command>;

/// Receiver used by the UI thread to read events emitted by the engine thread.
pub type EngineEventReceiver = mpsc::Receiver<Event>;

/// Messages emitted by the engine bridge subscription.
#[derive(Debug, Clone)]
pub enum BridgeEvent {
    Ready(EngineCommandSender),
    Event(Event),
    Disconnected,
}

/// Builds a subscription that starts the engine bridge and forwards events.
pub fn engine_subscription() -> Subscription<BridgeEvent> {
    Subscription::run(bridge_worker_stream)
}

fn bridge_worker_stream() -> impl iced::futures::Stream<Item = BridgeEvent> {
    stream::channel(SUBSCRIPTION_CHANNEL_CAPACITY, |mut output| async move {
        let (engine_tx, engine_rx) = spawn_ffmpeg_bridge();
        let _ = output.send(BridgeEvent::Ready(engine_tx)).await;

        let (forward_tx, mut forward_rx) =
            futures_mpsc::channel::<BridgeEvent>(SUBSCRIPTION_CHANNEL_CAPACITY);

        thread::spawn(move || {
            let mut forward_tx = forward_tx;
            while let Ok(event) = engine_rx.recv() {
                if executor::block_on(forward_tx.send(BridgeEvent::Event(event))).is_err() {
                    return;
                }
            }
            let _ = executor::block_on(forward_tx.send(BridgeEvent::Disconnected));
        });

        while let Some(event) = forward_rx.next().await {
            if output.send(event).await.is_err() {
                break;
            }
        }
    })
}

/// Spawns the production bridge that wires a FFmpeg-backed engine.
pub fn spawn_ffmpeg_bridge() -> (EngineCommandSender, EngineEventReceiver) {
    spawn_engine_bridge(Engine::with_ffmpeg())
}

/// Spawns a bridge around any engine backend.
pub fn spawn_engine_bridge<M>(mut engine: Engine<M>) -> (EngineCommandSender, EngineEventReceiver)
where
    M: MediaBackend + Send + 'static,
{
    let (command_tx, command_rx) = mpsc::sync_channel::<Command>(COMMAND_CHANNEL_CAPACITY);
    let (event_tx, event_rx) = mpsc::sync_channel::<Event>(EVENT_CHANNEL_CAPACITY);

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
                        .send(Event::Error(EngineErrorEvent {
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
