use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{EngineError, Result};
use crate::export::ExportVideoPlan;
use crate::project::ensure_non_empty_duration;
use crate::time::{Rational, TIMELINE_TIME_BASE, rescale};

/// Pixel format for preview frames passed to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewPixelFormat {
    Rgba8,
    Nv12,
}

/// Raw preview frame payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewFrame {
    pub width: u32,
    pub height: u32,
    pub format: PreviewPixelFormat,
    pub bytes: Arc<[u8]>,
}

/// Result of probing one media asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbedMedia {
    pub path: PathBuf,
    pub duration_tl: i64,
    pub video: Option<ProbedVideoStream>,
    pub audio: Option<ProbedAudioStream>,
}

/// Probed video stream information used by timeline mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbedVideoStream {
    pub time_base: Rational,
    pub src_in: i64,
    pub src_out: i64,
    pub width: u32,
    pub height: u32,
}

/// Probed audio stream information used by timeline mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbedAudioStream {
    pub time_base: Rational,
    pub src_in: i64,
    pub src_out: i64,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Media operations required by the engine.
pub trait MediaBackend {
    /// Probes media information for import.
    fn probe(&self, path: &Path) -> Result<ProbedMedia>;

    /// Decodes one preview frame around `at_seconds`.
    fn decode_preview_frame(&self, path: &Path, at_seconds: f64) -> Result<PreviewFrame>;

    /// Exports timeline segments into a single video-only file.
    fn export_video(&self, plan: &ExportVideoPlan) -> Result<()>;
}

/// FFmpeg CLI-backed backend used by production wiring.
#[derive(Debug, Default, Clone, Copy)]
pub struct FfmpegMediaBackend;

impl MediaBackend for FfmpegMediaBackend {
    fn probe(&self, path: &Path) -> Result<ProbedMedia> {
        let info = media_ffmpeg::probe_media(path)?;

        let duration_tl = ensure_non_empty_duration(path, duration_tl_from_probe(&info))?;
        let video = info
            .first_video()
            .map(|stream| -> Result<ProbedVideoStream> {
                let src_in = stream.start_pts.unwrap_or(0);
                let src_out = stream
                    .duration_ts
                    .map(|duration| src_in + duration)
                    .unwrap_or_else(|| {
                        src_in + rescale(duration_tl, TIMELINE_TIME_BASE, stream.time_base.into())
                    });
                Ok(ProbedVideoStream {
                    time_base: stream.time_base.into(),
                    src_in,
                    src_out,
                    width: stream
                        .width
                        .ok_or_else(|| EngineError::MissingVideoDimensions(path.to_path_buf()))?,
                    height: stream
                        .height
                        .ok_or_else(|| EngineError::MissingVideoDimensions(path.to_path_buf()))?,
                })
            })
            .transpose()?;

        let audio = info
            .first_audio()
            .map(|stream| -> Result<ProbedAudioStream> {
                let src_in = stream.start_pts.unwrap_or(0);
                let src_out = stream
                    .duration_ts
                    .map(|duration| src_in + duration)
                    .unwrap_or_else(|| {
                        src_in + rescale(duration_tl, TIMELINE_TIME_BASE, stream.time_base.into())
                    });

                let sample_rate = stream
                    .sample_rate
                    .ok_or_else(|| EngineError::MissingAudioMetadata(path.to_path_buf()))?;
                let channels = stream
                    .channels
                    .ok_or_else(|| EngineError::MissingAudioMetadata(path.to_path_buf()))?;

                Ok(ProbedAudioStream {
                    time_base: stream.time_base.into(),
                    src_in,
                    src_out,
                    sample_rate,
                    channels,
                })
            })
            .transpose()?;

        Ok(ProbedMedia {
            path: info.path,
            duration_tl,
            video,
            audio,
        })
    }

    fn decode_preview_frame(&self, path: &Path, at_seconds: f64) -> Result<PreviewFrame> {
        let decoded = media_ffmpeg::decode_video_frame_near_seconds(path, at_seconds)?;
        Ok(PreviewFrame {
            width: decoded.width,
            height: decoded.height,
            format: PreviewPixelFormat::Rgba8,
            bytes: decoded.rgba.into(),
        })
    }

    fn export_video(&self, plan: &ExportVideoPlan) -> Result<()> {
        let request = media_ffmpeg::VideoExportRequest {
            inputs: plan.inputs.clone(),
            segments: plan
                .segments
                .iter()
                .map(|segment| media_ffmpeg::VideoExportSegment {
                    input_index: segment.input_index,
                    src_in_video: segment.src_in_video,
                    src_out_video: segment.src_out_video,
                    src_time_base: segment.src_time_base.into(),
                })
                .collect(),
            output_path: plan.output_path.clone(),
        };
        media_ffmpeg::export_video_mp4(&request)?;
        Ok(())
    }
}

fn duration_tl_from_probe(info: &media_ffmpeg::MediaInfo) -> Option<i64> {
    if let Some(seconds) = info.duration_seconds {
        return Some((seconds * 1_000_000.0).round() as i64);
    }

    let mut best = None;
    for stream in &info.streams {
        let Some(duration_ts) = stream.duration_ts else {
            continue;
        };

        let tl = rescale(duration_ts, stream.time_base.into(), TIMELINE_TIME_BASE);
        best = Some(best.map_or(tl, |current: i64| current.max(tl)));
    }

    best
}
