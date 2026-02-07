use std::path::{Path, PathBuf};

use crate::api::{MediaAssetSummary, ProjectSnapshot, SegmentSummary};
use crate::error::{EngineError, Result};
use crate::preview::{ProbedAudioStream, ProbedMedia, ProbedVideoStream};
use crate::time::{TIMELINE_TIME_BASE, rescale};
use crate::timeline::{AssetId, Segment, SegmentId, Timeline};

/// Project state managed by the engine thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub assets: Vec<MediaAsset>,
    pub timeline: Timeline,
    pub settings: ProjectSettings,
}

/// Project-wide settings placeholder for future steps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectSettings {}

/// Imported media tracked by the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaAsset {
    pub id: AssetId,
    pub path: PathBuf,
    pub video: Option<VideoStreamInfo>,
    pub audio: Option<AudioStreamInfo>,
    pub duration_tl: i64,
}

/// Video metadata required by timeline mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoStreamInfo {
    pub time_base: crate::time::Rational,
    pub width: u32,
    pub height: u32,
}

/// Audio metadata required by timeline mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioStreamInfo {
    pub time_base: crate::time::Rational,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Preview request computed from timeline and source mapping.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewRequest {
    pub path: PathBuf,
    pub source_seconds: f64,
}

impl Project {
    /// Builds a project with a single segment spanning the full imported asset duration.
    pub fn from_single_asset(
        asset_id: AssetId,
        segment_id: SegmentId,
        probed: ProbedMedia,
    ) -> Result<Self> {
        let asset = MediaAsset {
            id: asset_id,
            path: probed.path.clone(),
            video: probed.video.map(VideoStreamInfo::from),
            audio: probed.audio.map(AudioStreamInfo::from),
            duration_tl: probed.duration_tl,
        };

        let segment = Segment {
            id: segment_id,
            asset_id,
            src_in_video: probed.video.as_ref().map(|video| video.src_in),
            src_out_video: probed.video.as_ref().map(|video| video.src_out),
            src_in_audio: probed.audio.as_ref().map(|audio| audio.src_in),
            src_out_audio: probed.audio.as_ref().map(|audio| audio.src_out),
            timeline_start: 0,
            timeline_duration: probed.duration_tl,
        };

        Ok(Self {
            assets: vec![asset],
            timeline: Timeline {
                segments: vec![segment],
            },
            settings: ProjectSettings::default(),
        })
    }

    /// Returns project duration in timeline ticks.
    pub fn duration_tl(&self) -> i64 {
        self.timeline.duration_tl()
    }

    /// Creates an immutable snapshot for the UI.
    pub fn snapshot(&self) -> ProjectSnapshot {
        ProjectSnapshot {
            assets: self
                .assets
                .iter()
                .map(|asset| MediaAssetSummary {
                    id: asset.id,
                    path: asset.path.clone(),
                    has_video: asset.video.is_some(),
                    has_audio: asset.audio.is_some(),
                    duration_tl: asset.duration_tl,
                })
                .collect(),
            segments: self
                .timeline
                .segments
                .iter()
                .map(|segment| SegmentSummary {
                    id: segment.id,
                    asset_id: segment.asset_id,
                    timeline_start: segment.timeline_start,
                    timeline_duration: segment.timeline_duration,
                    src_in_video: segment.src_in_video,
                    src_out_video: segment.src_out_video,
                    src_in_audio: segment.src_in_audio,
                    src_out_audio: segment.src_out_audio,
                })
                .collect(),
            duration_tl: self.duration_tl(),
        }
    }

    /// Computes the preview request for a timeline timestamp.
    pub fn preview_request_at(&self, t_tl: i64) -> Result<PreviewRequest> {
        let index = self
            .timeline
            .find_segment_index(t_tl)
            .ok_or(EngineError::SegmentNotFound { at_tl: t_tl })?;
        let segment = &self.timeline.segments[index];
        let asset = self.asset_by_id(segment.asset_id)?;
        let video = asset
            .video
            .ok_or(EngineError::MissingVideoStream { asset_id: asset.id })?;

        let src_in_video = segment
            .src_in_video
            .ok_or(EngineError::MissingVideoStream { asset_id: asset.id })?;
        let local_tl = t_tl - segment.timeline_start;
        let src_target_video_ts =
            src_in_video + rescale(local_tl, TIMELINE_TIME_BASE, video.time_base);
        let src_target_tl = rescale(src_target_video_ts, video.time_base, TIMELINE_TIME_BASE);
        let source_seconds = (src_target_tl.max(0)) as f64 / TIMELINE_TIME_BASE.den as f64;

        Ok(PreviewRequest {
            path: asset.path.clone(),
            source_seconds,
        })
    }

    /// Splits one segment at `at_tl`.
    ///
    /// The timeline remains contiguous on success. The operation fails when
    /// `at_tl` points to a segment boundary or to a position outside the
    /// current timeline.
    ///
    /// # Example
    /// ```ignore
    /// let mut project = /* construct project */;
    /// project.split(500_000, 2).unwrap();
    /// ```
    pub fn split(&mut self, at_tl: i64, next_segment_id: SegmentId) -> Result<()> {
        let index = match self.timeline.find_segment_index(at_tl) {
            Some(index) => index,
            None => {
                if self.timeline.is_boundary_split_point(at_tl) {
                    return Err(EngineError::SplitPointAtBoundary { at_tl });
                }

                return Err(EngineError::SegmentNotFound { at_tl });
            }
        };
        let segment = &self.timeline.segments[index];
        let asset = self.asset_by_id(segment.asset_id)?;

        self.timeline.split_segment(
            at_tl,
            next_segment_id,
            asset.video.map(|video| video.time_base),
            asset.audio.map(|audio| audio.time_base),
        )
    }

    fn asset_by_id(&self, asset_id: AssetId) -> Result<&MediaAsset> {
        self.assets
            .iter()
            .find(|asset| asset.id == asset_id)
            .ok_or(EngineError::MissingAsset { asset_id })
    }
}

impl From<ProbedVideoStream> for VideoStreamInfo {
    fn from(value: ProbedVideoStream) -> Self {
        Self {
            time_base: value.time_base,
            width: value.width,
            height: value.height,
        }
    }
}

impl From<ProbedAudioStream> for AudioStreamInfo {
    fn from(value: ProbedAudioStream) -> Self {
        Self {
            time_base: value.time_base,
            sample_rate: value.sample_rate,
            channels: value.channels,
        }
    }
}

pub(crate) fn normalize_playhead(t_tl: i64, duration_tl: i64) -> i64 {
    if duration_tl <= 0 {
        return 0;
    }

    let max_tick = duration_tl - 1;
    t_tl.clamp(0, max_tick)
}

pub(crate) fn ensure_non_empty_duration(path: &Path, duration_tl: Option<i64>) -> Result<i64> {
    let duration_tl =
        duration_tl.ok_or_else(|| EngineError::MissingDuration(path.to_path_buf()))?;
    Ok(duration_tl.max(1))
}
