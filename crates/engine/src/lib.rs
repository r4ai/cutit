//! UI-agnostic editing engine for the Cutit MVP.

pub mod api;
pub mod cache;
pub mod error;
pub mod export;
pub mod preview;
pub mod project;
pub mod time;
pub mod timeline;

pub use api::{
    Command, Engine, EngineErrorEvent, EngineErrorKind, Event, ExportSettings, ProjectSnapshot,
};
pub use error::{EngineError, Result};
pub use preview::{
    FfmpegMediaBackend, MediaBackend, PreviewFrame, PreviewPixelFormat, ProbedAudioStream,
    ProbedMedia, ProbedVideoStream,
};
pub use time::{Rational, TIMELINE_TIME_BASE, rescale};
