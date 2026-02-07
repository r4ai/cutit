use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use engine::{Command, Event};

/// Channel-backed bridge between UI state and engine worker.
#[derive(Debug)]
pub struct EngineBridge {
    command_tx: Sender<Command>,
    event_rx: Receiver<Event>,
}

impl EngineBridge {
    /// Creates a bridge from command sender and event receiver.
    pub fn new(command_tx: Sender<Command>, event_rx: Receiver<Event>) -> Self {
        Self {
            command_tx,
            event_rx,
        }
    }

    /// Sends one command to the engine worker.
    pub fn send_command(&self, command: Command) -> Result<(), BridgeError> {
        self.command_tx
            .send(command)
            .map_err(|_| BridgeError::Disconnected)
    }

    /// Receives all currently queued events without blocking.
    pub fn drain_events(&self) -> Result<Vec<Event>, BridgeError> {
        let mut events = Vec::new();
        loop {
            match self.event_rx.try_recv() {
                Ok(event) => events.push(event),
                Err(TryRecvError::Empty) => return Ok(events),
                Err(TryRecvError::Disconnected) => return Err(BridgeError::Disconnected),
            }
        }
    }
}

/// Error raised by the UI-engine bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeError {
    Disconnected,
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use engine::{Command, Event};

    use super::EngineBridge;

    #[test]
    fn sends_commands_and_drains_available_events() {
        let (command_tx, command_rx) = mpsc::channel::<Command>();
        let (event_tx, event_rx) = mpsc::channel::<Event>();
        let bridge = EngineBridge::new(command_tx, event_rx);

        bridge
            .send_command(Command::SetPlayhead { t_tl: 42 })
            .expect("command should be sent");
        event_tx
            .send(Event::PlayheadChanged { t_tl: 42 })
            .expect("event should be sent");

        assert_eq!(
            command_rx.recv().expect("command should be received"),
            Command::SetPlayhead { t_tl: 42 }
        );
        assert_eq!(
            bridge.drain_events().expect("events should be drained"),
            vec![Event::PlayheadChanged { t_tl: 42 }]
        );
    }
}
