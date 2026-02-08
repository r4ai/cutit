use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::api::{MediaAssetSummary, ProjectSnapshot, SegmentSummary};
use crate::error::{EngineError, Result};
use crate::preview::{ProbedAudioStream, ProbedMedia, ProbedVideoStream};
use crate::time::{Rational, TIMELINE_TIME_BASE, rescale};
use crate::timeline::{AssetId, Segment, SegmentId, Timeline};
use serde::{Deserialize, Serialize};

const PROJECT_FILE_SCHEMA_VERSION: u32 = 1;

/// Project state managed by the engine thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub assets: Vec<MediaAsset>,
    pub timeline: Timeline,
    pub settings: ProjectSettings,
}

/// Project-wide defaults and persisted settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_settings: Option<ProjectExportSettings>,
}

/// Optional export defaults persisted in a project file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectExportSettings {
    pub container: String,
    pub video_codec: String,
    pub audio_codec: String,
}

/// Imported media tracked by the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaAsset {
    pub id: AssetId,
    pub path: PathBuf,
    pub video_stream_index: Option<u32>,
    pub audio_stream_index: Option<u32>,
    pub video: Option<VideoStreamInfo>,
    pub audio: Option<AudioStreamInfo>,
    pub duration_tl: i64,
}

/// Video metadata required by timeline mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoStreamInfo {
    pub time_base: crate::time::Rational,
    pub width: u32,
    pub height: u32,
}

/// Audio metadata required by timeline mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioStreamInfo {
    pub time_base: crate::time::Rational,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Preview request computed from timeline and source mapping.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewRequest {
    pub path: PathBuf,
    pub source_tl: i64,
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
            video_stream_index: probed.video.as_ref().map(|video| video.stream_index),
            audio_stream_index: probed.audio.as_ref().map(|audio| audio.stream_index),
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

    /// Persists the current project to a JSON file.
    ///
    /// # Example
    /// ```ignore
    /// use std::path::PathBuf;
    ///
    /// let project = /* construct project */;
    /// project.save_to_file(PathBuf::from("project.nle.json")).unwrap();
    /// ```
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<()> {
        self.validate_for_persistence()?;

        let path = path.as_ref();
        let file = ProjectFile::from_project(self);
        let text = serde_json::to_string_pretty(&file).map_err(|source| {
            EngineError::ProjectSerialization {
                path: path.to_path_buf(),
                source,
            }
        })?;

        fs::write(path, text).map_err(|source| EngineError::ProjectIo {
            context: "write project file",
            path: path.to_path_buf(),
            source,
        })
    }

    /// Loads a project from a JSON file.
    ///
    /// # Example
    /// ```ignore
    /// use engine::project::Project;
    ///
    /// let project = Project::load_from_file("project.nle.json").unwrap();
    /// assert!(!project.assets.is_empty());
    /// ```
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|source| EngineError::ProjectIo {
            context: "read project file",
            path: path.to_path_buf(),
            source,
        })?;

        let file: ProjectFile =
            serde_json::from_str(&text).map_err(|source| EngineError::ProjectSerialization {
                path: path.to_path_buf(),
                source,
            })?;

        if file.schema_version != PROJECT_FILE_SCHEMA_VERSION {
            return Err(EngineError::InvalidProjectFile {
                reason: format!("unsupported project schema version {}", file.schema_version),
            });
        }

        let project = file.into_project();
        project.validate_for_persistence()?;
        Ok(project)
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
        let source_tl = src_target_tl.max(0);
        let source_seconds = source_tl as f64 / TIMELINE_TIME_BASE.den as f64;

        Ok(PreviewRequest {
            path: asset.path.clone(),
            source_tl,
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

    /// Cuts one segment at `at_tl` and keeps timeline gaps.
    ///
    /// When `at_tl` points to a segment start boundary, the segment starting
    /// at that boundary is removed.
    ///
    /// # Example
    /// ```ignore
    /// let mut project = /* construct project */;
    /// project.cut(500_000).unwrap();
    /// ```
    pub fn cut(&mut self, at_tl: i64) -> Result<()> {
        let _ = self.timeline.cut_segment(at_tl)?;
        Ok(())
    }

    /// Moves one segment start time without changing its source range.
    ///
    /// The move is clamped so that segment order stays stable and no overlap is
    /// introduced with adjacent segments.
    pub fn move_segment(&mut self, segment_id: SegmentId, new_start_tl: i64) -> Result<()> {
        let index = self
            .timeline
            .find_segment_index_by_id(segment_id)
            .ok_or(EngineError::SegmentIdNotFound { segment_id })?;

        let prev_end = if index == 0 {
            0
        } else {
            let prev = &self.timeline.segments[index - 1];
            prev.timeline_start.saturating_add(prev.timeline_duration)
        };
        let duration = self.timeline.segments[index].timeline_duration;
        let max_start = if index + 1 < self.timeline.segments.len() {
            self.timeline.segments[index + 1]
                .timeline_start
                .saturating_sub(duration)
        } else {
            i64::MAX.saturating_sub(duration.max(0))
        };
        let clamped = new_start_tl.max(0).clamp(prev_end, max_start.max(prev_end));
        self.timeline.segments[index].timeline_start = clamped;
        Ok(())
    }

    /// Trims the start edge of one segment.
    pub fn trim_segment_start(&mut self, segment_id: SegmentId, new_start_tl: i64) -> Result<()> {
        let index = self
            .timeline
            .find_segment_index_by_id(segment_id)
            .ok_or(EngineError::SegmentIdNotFound { segment_id })?;
        let segment = &self.timeline.segments[index];
        let asset = self.asset_by_id(segment.asset_id)?;
        let video_tb = asset.video.map(|video| video.time_base);
        let audio_tb = asset.audio.map(|audio| audio.time_base);

        let prev_end = if index == 0 {
            0
        } else {
            let prev = &self.timeline.segments[index - 1];
            prev.timeline_start + prev.timeline_duration
        };
        let old_start = segment.timeline_start;
        let old_end = old_start + segment.timeline_duration;
        let clamped_start = new_start_tl.clamp(prev_end, old_end - 1);
        let delta_tl = clamped_start - old_start;

        let segment = &mut self.timeline.segments[index];
        segment.timeline_start = clamped_start;
        segment.timeline_duration = old_end - clamped_start;
        segment.src_in_video = shift_stream_point(segment.src_in_video, delta_tl, video_tb);
        segment.src_in_audio = shift_stream_point(segment.src_in_audio, delta_tl, audio_tb);
        Ok(())
    }

    /// Trims the end edge of one segment.
    pub fn trim_segment_end(&mut self, segment_id: SegmentId, new_end_tl: i64) -> Result<()> {
        let index = self
            .timeline
            .find_segment_index_by_id(segment_id)
            .ok_or(EngineError::SegmentIdNotFound { segment_id })?;
        let segment = &self.timeline.segments[index];
        let asset = self.asset_by_id(segment.asset_id)?;
        let video_tb = asset.video.map(|video| video.time_base);
        let audio_tb = asset.audio.map(|audio| audio.time_base);

        let old_start = segment.timeline_start;
        let old_end = old_start + segment.timeline_duration;
        let next_start = if index + 1 < self.timeline.segments.len() {
            self.timeline.segments[index + 1].timeline_start
        } else {
            i64::MAX
        };
        let clamped_end = new_end_tl.clamp(old_start + 1, next_start);
        let delta_tl = clamped_end - old_end;

        let segment = &mut self.timeline.segments[index];
        segment.timeline_duration = clamped_end - old_start;
        segment.src_out_video = shift_stream_point(segment.src_out_video, delta_tl, video_tb);
        segment.src_out_audio = shift_stream_point(segment.src_out_audio, delta_tl, audio_tb);
        Ok(())
    }

    fn asset_by_id(&self, asset_id: AssetId) -> Result<&MediaAsset> {
        self.assets
            .iter()
            .find(|asset| asset.id == asset_id)
            .ok_or(EngineError::MissingAsset { asset_id })
    }

    fn validate_for_persistence(&self) -> Result<()> {
        let mut seen_asset_ids = HashSet::new();
        for asset in &self.assets {
            if !seen_asset_ids.insert(asset.id) {
                return Err(EngineError::InvalidProjectFile {
                    reason: format!("duplicate asset id {}", asset.id),
                });
            }

            if asset.video.is_some() && asset.video_stream_index.is_none() {
                return Err(EngineError::InvalidProjectFile {
                    reason: format!("asset {} is missing video stream index", asset.id),
                });
            }
            if asset.video.is_none() && asset.video_stream_index.is_some() {
                return Err(EngineError::InvalidProjectFile {
                    reason: format!(
                        "asset {} has video stream index without video stream",
                        asset.id
                    ),
                });
            }

            if asset.audio.is_some() && asset.audio_stream_index.is_none() {
                return Err(EngineError::InvalidProjectFile {
                    reason: format!("asset {} is missing audio stream index", asset.id),
                });
            }
            if asset.audio.is_none() && asset.audio_stream_index.is_some() {
                return Err(EngineError::InvalidProjectFile {
                    reason: format!(
                        "asset {} has audio stream index without audio stream",
                        asset.id
                    ),
                });
            }
        }

        let mut previous_end: Option<i64> = None;
        let mut seen_segment_ids = HashSet::new();
        for segment in &self.timeline.segments {
            if !seen_segment_ids.insert(segment.id) {
                return Err(EngineError::InvalidProjectFile {
                    reason: format!("duplicate segment id {}", segment.id),
                });
            }

            if segment.timeline_start < 0 {
                return Err(EngineError::InvalidProjectFile {
                    reason: format!(
                        "segment {} starts at negative timeline {}",
                        segment.id, segment.timeline_start
                    ),
                });
            }
            if segment.timeline_duration <= 0 {
                return Err(EngineError::InvalidProjectFile {
                    reason: format!(
                        "segment {} has non-positive duration {}",
                        segment.id, segment.timeline_duration
                    ),
                });
            }

            let asset = self.asset_by_id(segment.asset_id)?;
            validate_segment_ranges(asset, segment)?;

            let segment_end = segment
                .timeline_start
                .checked_add(segment.timeline_duration)
                .ok_or_else(|| EngineError::InvalidProjectFile {
                    reason: format!("segment {} timeline range overflowed i64", segment.id),
                })?;
            if let Some(previous_end) = previous_end
                && segment.timeline_start < previous_end
            {
                return Err(EngineError::InvalidProjectFile {
                    reason: format!(
                        "segment {} overlaps previous segment at {}",
                        segment.id, segment.timeline_start
                    ),
                });
            }
            previous_end = Some(segment_end);
        }

        Ok(())
    }
}

fn shift_stream_point(
    point: Option<i64>,
    delta_tl: i64,
    time_base: Option<Rational>,
) -> Option<i64> {
    let (Some(point), Some(time_base)) = (point, time_base) else {
        return point;
    };
    let delta_stream = rescale(delta_tl, TIMELINE_TIME_BASE, time_base);
    let shifted = point.saturating_add(delta_stream);
    Some(shifted.max(0))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ProjectFile {
    schema_version: u32,
    assets: Vec<MediaAsset>,
    segments: Vec<Segment>,
    #[serde(default)]
    settings: ProjectSettings,
}

impl ProjectFile {
    fn from_project(project: &Project) -> Self {
        Self {
            schema_version: PROJECT_FILE_SCHEMA_VERSION,
            assets: project.assets.clone(),
            segments: project.timeline.segments.clone(),
            settings: project.settings.clone(),
        }
    }

    fn into_project(self) -> Project {
        Project {
            assets: self.assets,
            timeline: Timeline {
                segments: self.segments,
            },
            settings: self.settings,
        }
    }
}

fn validate_segment_ranges(asset: &MediaAsset, segment: &Segment) -> Result<()> {
    validate_video_segment_range(asset, segment)?;
    validate_audio_segment_range(asset, segment)?;
    Ok(())
}

fn validate_video_segment_range(asset: &MediaAsset, segment: &Segment) -> Result<()> {
    match (asset.video, segment.src_in_video, segment.src_out_video) {
        (Some(_), Some(src_in_video), Some(src_out_video)) => {
            if src_out_video < src_in_video {
                return Err(EngineError::InvalidVideoRange {
                    segment_id: segment.id,
                    src_in_video,
                    src_out_video,
                });
            }
            Ok(())
        }
        (Some(_), _, _) => Err(EngineError::MissingVideoRange {
            segment_id: segment.id,
        }),
        (None, None, None) => Ok(()),
        (None, _, _) => Err(EngineError::MissingVideoStream { asset_id: asset.id }),
    }
}

fn validate_audio_segment_range(asset: &MediaAsset, segment: &Segment) -> Result<()> {
    match (asset.audio, segment.src_in_audio, segment.src_out_audio) {
        (Some(_), Some(src_in_audio), Some(src_out_audio)) => {
            if src_out_audio < src_in_audio {
                return Err(EngineError::InvalidAudioRange {
                    segment_id: segment.id,
                    src_in_audio,
                    src_out_audio,
                });
            }
            Ok(())
        }
        (Some(_), _, _) => Err(EngineError::MissingAudioRange {
            segment_id: segment.id,
        }),
        (None, None, None) => Ok(()),
        (None, _, _) => Err(EngineError::MissingAudioStream { asset_id: asset.id }),
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        AudioStreamInfo, MediaAsset, Project, ProjectExportSettings, ProjectSettings,
        VideoStreamInfo, normalize_playhead,
    };
    use crate::error::EngineError;
    use crate::time::Rational;
    use crate::timeline::{Segment, Timeline};

    #[test]
    fn project_persistence_roundtrip_restores_assets_segments_and_settings() {
        let project = sample_project();
        let path = temp_file_path("project-roundtrip", "json");

        project.save_to_file(&path).expect("save should succeed");
        let loaded = Project::load_from_file(&path).expect("load should succeed");

        assert_eq!(loaded, project);
        fs::remove_file(path).expect("cleanup persisted file");
    }

    #[test]
    fn project_persistence_json_includes_stream_selection_indices_and_export_settings() {
        let project = sample_project();
        let path = temp_file_path("project-schema", "json");

        project.save_to_file(&path).expect("save should succeed");
        let text = fs::read_to_string(&path).expect("persisted json must be readable");
        let json: serde_json::Value = serde_json::from_str(&text).expect("json should be valid");

        assert_eq!(
            json["assets"][0]["video_stream_index"],
            serde_json::json!(3)
        );
        assert_eq!(
            json["assets"][0]["audio_stream_index"],
            serde_json::json!(7)
        );
        assert_eq!(
            json["settings"]["export_settings"]["container"],
            serde_json::json!("mp4")
        );
        fs::remove_file(path).expect("cleanup persisted file");
    }

    #[test]
    fn normalize_playhead_returns_zero_for_empty_project_duration() {
        assert_eq!(normalize_playhead(10, 0), 0);
    }

    #[test]
    fn move_segment_clamps_last_segment_to_prevent_timeline_overflow() {
        let mut project = sample_project();
        let duration = project.timeline.segments[0].timeline_duration;

        project
            .move_segment(1, i64::MAX)
            .expect("move should succeed");

        let moved = &project.timeline.segments[0];
        assert_eq!(moved.timeline_start, i64::MAX - duration);
        assert_eq!(project.timeline.duration_tl(), i64::MAX);
    }

    #[test]
    fn trim_segment_start_clamps_shifted_stream_points_to_zero() {
        let mut project = sample_project();
        let segment = &mut project.timeline.segments[0];
        segment.timeline_start = 100;
        segment.src_in_video = Some(5);
        segment.src_out_video = Some(50);
        segment.src_in_audio = Some(3);
        segment.src_out_audio = Some(30);

        project
            .trim_segment_start(1, 0)
            .expect("trim start should succeed");

        let trimmed = &project.timeline.segments[0];
        assert_eq!(trimmed.src_in_video, Some(0));
        assert_eq!(trimmed.src_out_video, Some(50));
        assert_eq!(trimmed.src_in_audio, Some(0));
        assert_eq!(trimmed.src_out_audio, Some(30));
    }

    #[test]
    fn project_persistence_allows_zero_length_stream_ranges() {
        let mut project = sample_project();
        let segment = &mut project.timeline.segments[0];
        segment.src_out_video = segment.src_in_video;
        segment.src_out_audio = segment.src_in_audio;
        let path = temp_file_path("project-zero-length-ranges", "json");

        project.save_to_file(&path).expect("save should succeed");
        let loaded = Project::load_from_file(&path).expect("load should succeed");

        assert_eq!(loaded.timeline.segments[0].src_in_video, Some(90_000));
        assert_eq!(loaded.timeline.segments[0].src_out_video, Some(90_000));
        assert_eq!(loaded.timeline.segments[0].src_in_audio, Some(48_000));
        assert_eq!(loaded.timeline.segments[0].src_out_audio, Some(48_000));
        fs::remove_file(path).expect("cleanup persisted file");
    }

    #[test]
    fn project_persistence_allows_gaps_between_segments() {
        let mut project = sample_project();
        project.timeline.segments.push(Segment {
            id: 2,
            asset_id: 1,
            src_in_video: Some(180_000),
            src_out_video: Some(198_000),
            src_in_audio: Some(96_000),
            src_out_audio: Some(105_600),
            timeline_start: 1_500_000,
            timeline_duration: 200_000,
        });
        let path = temp_file_path("project-gap-segments", "json");

        project.save_to_file(&path).expect("save should succeed");
        let loaded = Project::load_from_file(&path).expect("load should succeed");

        assert_eq!(loaded.timeline.segments.len(), 2);
        assert_eq!(loaded.timeline.segments[1].timeline_start, 1_500_000);
        fs::remove_file(path).expect("cleanup persisted file");
    }

    #[test]
    fn project_persistence_rejects_duplicate_segment_ids() {
        let mut project = sample_project();
        project.timeline.segments.push(Segment {
            id: 1,
            asset_id: 1,
            src_in_video: Some(198_000),
            src_out_video: Some(198_001),
            src_in_audio: Some(105_600),
            src_out_audio: Some(105_601),
            timeline_start: 1_200_000,
            timeline_duration: 1,
        });

        let result = project.save_to_file(temp_file_path("duplicate-segment-id", "json"));
        assert!(matches!(
            result,
            Err(EngineError::InvalidProjectFile { .. })
        ));
    }

    #[test]
    fn load_project_rejects_invalid_rational_in_json() {
        let path = temp_file_path("invalid-rational-project", "json");
        let invalid = serde_json::json!({
            "schema_version": 1,
            "assets": [{
                "id": 1,
                "path": "assets/demo.mp4",
                "video_stream_index": 3,
                "audio_stream_index": 7,
                "video": {
                    "time_base": { "num": 1, "den": 0 },
                    "width": 1920,
                    "height": 1080
                },
                "audio": {
                    "time_base": { "num": 1, "den": 48000 },
                    "sample_rate": 48000,
                    "channels": 2
                },
                "duration_tl": 1200000
            }],
            "segments": [{
                "id": 1,
                "asset_id": 1,
                "src_in_video": 90000,
                "src_out_video": 198000,
                "src_in_audio": 48000,
                "src_out_audio": 105600,
                "timeline_start": 0,
                "timeline_duration": 1200000
            }],
            "settings": {
                "export_settings": {
                    "container": "mp4",
                    "video_codec": "h264",
                    "audio_codec": "aac"
                }
            }
        });
        fs::write(
            &path,
            serde_json::to_string_pretty(&invalid).expect("valid json"),
        )
        .expect("write invalid project json");

        let result = Project::load_from_file(&path);
        assert!(matches!(
            result,
            Err(EngineError::ProjectSerialization { .. })
        ));
        fs::remove_file(path).expect("cleanup persisted file");
    }

    fn sample_project() -> Project {
        Project {
            assets: vec![MediaAsset {
                id: 1,
                path: PathBuf::from("assets/demo.mp4"),
                video_stream_index: Some(3),
                audio_stream_index: Some(7),
                video: Some(VideoStreamInfo {
                    time_base: Rational::new(1, 90_000).expect("valid rational"),
                    width: 1920,
                    height: 1080,
                }),
                audio: Some(AudioStreamInfo {
                    time_base: Rational::new(1, 48_000).expect("valid rational"),
                    sample_rate: 48_000,
                    channels: 2,
                }),
                duration_tl: 1_200_000,
            }],
            timeline: Timeline {
                segments: vec![Segment {
                    id: 1,
                    asset_id: 1,
                    src_in_video: Some(90_000),
                    src_out_video: Some(198_000),
                    src_in_audio: Some(48_000),
                    src_out_audio: Some(105_600),
                    timeline_start: 0,
                    timeline_duration: 1_200_000,
                }],
            },
            settings: ProjectSettings {
                export_settings: Some(ProjectExportSettings {
                    container: String::from("mp4"),
                    video_codec: String::from("h264"),
                    audio_codec: String::from("aac"),
                }),
            },
        }
    }

    fn temp_file_path(prefix: &str, extension: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be monotonic")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}.{extension}"))
    }
}
