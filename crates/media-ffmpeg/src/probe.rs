use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{MediaFfmpegError, Result};
use crate::time::Rational;

/// Stream kind discovered by probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Video,
    Audio,
    Other,
}

/// Stream metadata read from `ffprobe`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    pub index: u32,
    pub kind: StreamKind,
    pub codec_name: Option<String>,
    pub time_base: Rational,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub r_frame_rate: Option<Rational>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub channel_layout: Option<String>,
    pub start_pts: Option<i64>,
    pub duration_ts: Option<i64>,
}

/// Media probe result.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    pub path: PathBuf,
    pub streams: Vec<StreamInfo>,
    pub duration_seconds: Option<f64>,
}

impl MediaInfo {
    /// Returns the first video stream.
    ///
    /// # Example
    /// ```no_run
    /// use media_ffmpeg::probe_media;
    ///
    /// let info = probe_media("sample.mp4").expect("probe should succeed");
    /// let _video = info.first_video().expect("video stream exists");
    /// ```
    pub fn first_video(&self) -> Option<&StreamInfo> {
        self.streams
            .iter()
            .find(|stream| stream.kind == StreamKind::Video)
    }

    /// Returns the first audio stream.
    ///
    /// # Example
    /// ```no_run
    /// use media_ffmpeg::probe_media;
    ///
    /// let info = probe_media("sample.mp4").expect("probe should succeed");
    /// let _audio = info.first_audio().expect("audio stream exists");
    /// ```
    pub fn first_audio(&self) -> Option<&StreamInfo> {
        self.streams
            .iter()
            .find(|stream| stream.kind == StreamKind::Audio)
    }
}

/// Probes a media file via `ffprobe`.
///
/// # Example
/// ```no_run
/// use media_ffmpeg::probe_media;
///
/// let info = probe_media("sample.mp4").expect("probe should succeed");
/// assert!(!info.streams.is_empty());
/// ```
pub fn probe_media(path: impl AsRef<Path>) -> Result<MediaInfo> {
    let path = path.as_ref();

    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=index,codec_type,codec_name,time_base,width,height,r_frame_rate,pix_fmt,sample_rate,channels,channel_layout,start_pts,duration_ts",
            "-of",
            "compact=p=0:nk=0",
        ])
        .arg(path)
        .output()
        .map_err(|source| MediaFfmpegError::Io {
            context: "run ffprobe stream probe",
            source,
        })?;

    if !output.status.success() {
        return Err(MediaFfmpegError::CommandFailed {
            command: command_for_display("ffprobe stream probe", path),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let stdout = String::from_utf8(output.stdout)?;
    let mut streams = Vec::new();
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        streams.push(parse_stream_line(line)?);
    }

    if streams.is_empty() {
        return Err(MediaFfmpegError::Parse {
            context: "streams",
            value: "no streams found".to_string(),
        });
    }

    let duration_seconds = probe_duration_seconds(path)?;
    Ok(MediaInfo {
        path: path.to_path_buf(),
        streams,
        duration_seconds,
    })
}

fn parse_stream_line(line: &str) -> Result<StreamInfo> {
    let mut map = HashMap::<&str, &str>::new();
    for field in line.split('|') {
        let (key, value) = field
            .split_once('=')
            .ok_or_else(|| MediaFfmpegError::Parse {
                context: "stream field",
                value: field.to_string(),
            })?;
        map.insert(key.trim(), unquote(value.trim()));
    }

    let codec_type = map
        .get("codec_type")
        .copied()
        .ok_or_else(|| MediaFfmpegError::Parse {
            context: "codec_type",
            value: line.to_string(),
        })?;
    let kind = match codec_type {
        "video" => StreamKind::Video,
        "audio" => StreamKind::Audio,
        _ => StreamKind::Other,
    };

    let index =
        parse_optional_u32(map.get("index").copied(), "stream index")?.ok_or_else(|| {
            MediaFfmpegError::Parse {
                context: "stream index",
                value: line.to_string(),
            }
        })?;
    let time_base = parse_optional_rational(map.get("time_base").copied(), "time_base")?
        .ok_or_else(|| MediaFfmpegError::Parse {
            context: "time_base",
            value: line.to_string(),
        })?;

    Ok(StreamInfo {
        index,
        kind,
        codec_name: map.get("codec_name").map(|value| value.to_string()),
        time_base,
        width: parse_optional_u32(map.get("width").copied(), "width")?,
        height: parse_optional_u32(map.get("height").copied(), "height")?,
        r_frame_rate: parse_optional_rational(map.get("r_frame_rate").copied(), "r_frame_rate")?,
        sample_rate: parse_optional_u32(map.get("sample_rate").copied(), "sample_rate")?,
        channels: parse_optional_u16(map.get("channels").copied(), "channels")?,
        channel_layout: map
            .get("channel_layout")
            .filter(|value| !value.is_empty() && **value != "N/A")
            .map(|value| value.to_string()),
        start_pts: parse_optional_i64(map.get("start_pts").copied(), "start_pts")?,
        duration_ts: parse_optional_i64(map.get("duration_ts").copied(), "duration_ts")?,
    })
}

fn probe_duration_seconds(path: &Path) -> Result<Option<f64>> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=nokey=1:noprint_wrappers=1",
        ])
        .arg(path)
        .output()
        .map_err(|source| MediaFfmpegError::Io {
            context: "run ffprobe duration probe",
            source,
        })?;

    if !output.status.success() {
        return Err(MediaFfmpegError::CommandFailed {
            command: command_for_display("ffprobe duration probe", path),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let stdout = String::from_utf8(output.stdout)?;
    let value = stdout.trim();
    if value.is_empty() || value == "N/A" {
        return Ok(None);
    }
    let duration = value.parse::<f64>().map_err(|_| MediaFfmpegError::Parse {
        context: "format duration seconds",
        value: value.to_string(),
    })?;
    Ok(Some(duration))
}

fn parse_optional_u32(value: Option<&str>, context: &'static str) -> Result<Option<u32>> {
    parse_optional(value, context, str::parse::<u32>)
}

fn parse_optional_u16(value: Option<&str>, context: &'static str) -> Result<Option<u16>> {
    parse_optional(value, context, str::parse::<u16>)
}

fn parse_optional_i64(value: Option<&str>, context: &'static str) -> Result<Option<i64>> {
    parse_optional(value, context, str::parse::<i64>)
}

fn parse_optional_rational(value: Option<&str>, context: &'static str) -> Result<Option<Rational>> {
    let Some(raw) = value else {
        return Ok(None);
    };
    if raw.is_empty() || raw == "N/A" || raw == "0/0" {
        return Ok(None);
    }

    Rational::parse(raw)
        .map(Some)
        .map_err(|_| MediaFfmpegError::Parse {
            context,
            value: raw.to_string(),
        })
}

fn parse_optional<T, F>(value: Option<&str>, context: &'static str, parse: F) -> Result<Option<T>>
where
    F: Fn(&str) -> std::result::Result<T, std::num::ParseIntError>,
{
    let Some(raw) = value else {
        return Ok(None);
    };
    if raw.is_empty() || raw == "N/A" {
        return Ok(None);
    }

    parse(raw).map(Some).map_err(|_| MediaFfmpegError::Parse {
        context,
        value: raw.to_string(),
    })
}

fn unquote(value: &str) -> &str {
    value.trim_matches('"')
}

fn command_for_display(context: &str, path: &Path) -> String {
    format!("{context}: ffprobe {}", path.display())
}
