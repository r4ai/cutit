use std::fmt::{Display, Formatter};
use std::path::PathBuf;

/// Result type used by this crate.
pub type Result<T> = std::result::Result<T, MediaFfmpegError>;

/// Error type for media probing/decoding operations backed by FFmpeg CLI tools.
#[derive(Debug)]
pub enum MediaFfmpegError {
    InvalidRational {
        num: i32,
        den: i32,
    },
    InvalidTimestampSeconds(f64),
    MissingVideoStream(PathBuf),
    MissingVideoDimensions(PathBuf),
    InvalidExportRequest {
        reason: &'static str,
    },
    Io {
        context: &'static str,
        source: std::io::Error,
    },
    CommandFailed {
        command: String,
        status: std::process::ExitStatus,
        stderr: String,
    },
    Utf8(std::string::FromUtf8Error),
    Parse {
        context: &'static str,
        value: String,
    },
}

impl Display for MediaFfmpegError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRational { num, den } => {
                write!(f, "invalid rational {num}/{den}")
            }
            Self::InvalidTimestampSeconds(value) => {
                write!(f, "invalid timestamp seconds: {value}")
            }
            Self::MissingVideoStream(path) => {
                write!(f, "video stream not found: {}", path.display())
            }
            Self::MissingVideoDimensions(path) => {
                write!(f, "video dimensions missing: {}", path.display())
            }
            Self::InvalidExportRequest { reason } => {
                write!(f, "invalid export request: {reason}")
            }
            Self::Io { context, source } => {
                write!(f, "{context}: {source}")
            }
            Self::CommandFailed {
                command,
                status,
                stderr,
            } => {
                write!(
                    f,
                    "command failed ({status}): {command}; stderr: {}",
                    stderr.trim()
                )
            }
            Self::Utf8(err) => write!(f, "utf8 decode error: {err}"),
            Self::Parse { context, value } => {
                write!(f, "parse error ({context}): {value}")
            }
        }
    }
}

impl std::error::Error for MediaFfmpegError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Utf8(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::string::FromUtf8Error> for MediaFfmpegError {
    fn from(value: std::string::FromUtf8Error) -> Self {
        Self::Utf8(value)
    }
}
