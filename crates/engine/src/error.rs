use std::fmt::{Display, Formatter};
use std::path::PathBuf;

/// Result type used by the engine crate.
pub type Result<T> = std::result::Result<T, EngineError>;

/// Errors produced by engine commands and timeline operations.
#[derive(Debug)]
pub enum EngineError {
    ProjectNotLoaded,
    SegmentNotFound {
        at_tl: i64,
    },
    SplitPointAtBoundary {
        at_tl: i64,
    },
    MissingAsset {
        asset_id: u64,
    },
    MissingVideoStream {
        asset_id: u64,
    },
    MissingVideoRange {
        segment_id: u64,
    },
    InvalidVideoRange {
        segment_id: u64,
        src_in_video: i64,
        src_out_video: i64,
    },
    MissingDuration(PathBuf),
    MissingVideoDimensions(PathBuf),
    MissingAudioMetadata(PathBuf),
    InvalidRational {
        num: i32,
        den: i32,
    },
    Media(media_ffmpeg::MediaFfmpegError),
}

impl Display for EngineError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProjectNotLoaded => write!(f, "project is not loaded"),
            Self::SegmentNotFound { at_tl } => {
                write!(f, "segment not found at timeline timestamp {at_tl}")
            }
            Self::SplitPointAtBoundary { at_tl } => {
                write!(f, "cannot split at segment boundary: {at_tl}")
            }
            Self::MissingAsset { asset_id } => write!(f, "asset not found: {asset_id}"),
            Self::MissingVideoStream { asset_id } => {
                write!(f, "video stream missing in asset {asset_id}")
            }
            Self::MissingVideoRange { segment_id } => {
                write!(f, "video range missing in segment {segment_id}")
            }
            Self::InvalidVideoRange {
                segment_id,
                src_in_video,
                src_out_video,
            } => write!(
                f,
                "invalid video range in segment {segment_id}: {src_in_video}..{src_out_video}"
            ),
            Self::MissingDuration(path) => {
                write!(f, "media duration is missing: {}", path.display())
            }
            Self::MissingVideoDimensions(path) => {
                write!(f, "video dimensions are missing: {}", path.display())
            }
            Self::MissingAudioMetadata(path) => {
                write!(f, "audio metadata is missing: {}", path.display())
            }
            Self::InvalidRational { num, den } => write!(f, "invalid rational {num}/{den}"),
            Self::Media(err) => write!(f, "media backend error: {err}"),
        }
    }
}

impl std::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Media(err) => Some(err),
            _ => None,
        }
    }
}

impl From<media_ffmpeg::MediaFfmpegError> for EngineError {
    fn from(value: media_ffmpeg::MediaFfmpegError) -> Self {
        Self::Media(value)
    }
}
