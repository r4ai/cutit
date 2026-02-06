use std::path::Path;
use std::process::Command;

use crate::error::{MediaFfmpegError, Result};
use crate::probe::probe_media;
use crate::time::{Rational, rescale};

/// A decoded video frame in RGBA format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedVideoFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub best_effort_timestamp: i64,
    pub time_base: Rational,
}

/// Decodes a single video frame at-or-after the requested timestamp.
///
/// This function resolves the target timestamp in the input stream time base
/// and picks the first `best_effort_timestamp >= target`.
///
/// # Example
/// ```no_run
/// use media_ffmpeg::decode_video_frame_near_seconds;
///
/// let frame = decode_video_frame_near_seconds("sample.mp4", 0.5)
///     .expect("decode should succeed");
/// assert!(!frame.rgba.is_empty());
/// ```
pub fn decode_video_frame_near_seconds(
    path: impl AsRef<Path>,
    at_seconds: f64,
) -> Result<DecodedVideoFrame> {
    if !at_seconds.is_finite() || at_seconds < 0.0 {
        return Err(MediaFfmpegError::InvalidTimestampSeconds(at_seconds));
    }

    let path = path.as_ref();
    let media = probe_media(path)?;
    let video = media
        .first_video()
        .ok_or_else(|| MediaFfmpegError::MissingVideoStream(path.to_path_buf()))?;
    let width = video
        .width
        .ok_or_else(|| MediaFfmpegError::MissingVideoDimensions(path.to_path_buf()))?;
    let height = video
        .height
        .ok_or_else(|| MediaFfmpegError::MissingVideoDimensions(path.to_path_buf()))?;
    let time_base = video.time_base;

    let target_tl = (at_seconds * 1_000_000.0).round() as i64;
    let target_video_ts = rescale(target_tl, Rational::MICROS, time_base);

    let timestamps = read_video_best_effort_timestamps(path)?;
    let best_effort_timestamp = select_timestamp_at_or_after(&timestamps, target_video_ts)
        .ok_or_else(|| MediaFfmpegError::Parse {
            context: "best_effort_timestamp",
            value: "no video frames found".to_string(),
        })?;

    let rgba = decode_rgba_frame_at_or_after(path, best_effort_timestamp)?;
    let expected_size = width as usize * height as usize * 4;
    if rgba.len() != expected_size {
        return Err(MediaFfmpegError::Parse {
            context: "decoded rgba size",
            value: format!("expected {expected_size} bytes, got {}", rgba.len()),
        });
    }

    Ok(DecodedVideoFrame {
        width,
        height,
        rgba,
        best_effort_timestamp,
        time_base,
    })
}

fn read_video_best_effort_timestamps(path: &Path) -> Result<Vec<i64>> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_frames",
            "-show_entries",
            "frame=best_effort_timestamp",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .map_err(|source| MediaFfmpegError::Io {
            context: "run ffprobe show_frames",
            source,
        })?;

    if !output.status.success() {
        return Err(MediaFfmpegError::CommandFailed {
            command: format!("ffprobe show_frames {}", path.display()),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let stdout = String::from_utf8(output.stdout)?;
    let mut timestamps = Vec::new();
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let Some(raw_ts) = line.split(',').next() else {
            continue;
        };
        if raw_ts.is_empty() || raw_ts == "N/A" {
            continue;
        }
        let ts = raw_ts.parse::<i64>().map_err(|_| MediaFfmpegError::Parse {
            context: "best_effort_timestamp",
            value: raw_ts.to_string(),
        })?;
        timestamps.push(ts);
    }
    Ok(timestamps)
}

fn select_timestamp_at_or_after(timestamps: &[i64], target: i64) -> Option<i64> {
    timestamps
        .iter()
        .copied()
        .find(|timestamp| *timestamp >= target)
        .or_else(|| timestamps.last().copied())
}

fn decode_rgba_frame_at_or_after(path: &Path, timestamp: i64) -> Result<Vec<u8>> {
    let filter = format!("select=gte(pts\\,{timestamp}),format=rgba");
    let output = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-v")
        .arg("error")
        .arg("-i")
        .arg(path)
        .arg("-vf")
        .arg(&filter)
        .arg("-frames:v")
        .arg("1")
        .arg("-f")
        .arg("rawvideo")
        .arg("-pix_fmt")
        .arg("rgba")
        .arg("-")
        .output()
        .map_err(|source| MediaFfmpegError::Io {
            context: "run ffmpeg decode frame",
            source,
        })?;

    if !output.status.success() {
        return Err(MediaFfmpegError::CommandFailed {
            command: format!("ffmpeg decode frame {}", path.display()),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    Ok(output.stdout)
}
