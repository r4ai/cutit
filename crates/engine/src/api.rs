use std::path::PathBuf;

use crate::error::{EngineError, Result};
use crate::export::build_video_export_plan;
use crate::preview::{FfmpegMediaBackend, MediaBackend, PreviewFrame};
use crate::project::{Project, normalize_playhead};
use tracing::info;

/// Commands accepted by the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Import {
        path: PathBuf,
    },
    SetPlayhead {
        t_tl: i64,
    },
    /// Splits the segment at `at_tl` in timeline ticks.
    ///
    /// # Example
    /// ```ignore
    /// use std::path::PathBuf;
    /// use engine::{Command, Engine, FfmpegMediaBackend};
    ///
    /// let mut engine = Engine::new(FfmpegMediaBackend);
    /// let _ = engine.handle_command(Command::Import {
    ///     path: PathBuf::from("demo.mp4"),
    /// });
    /// let _ = engine.handle_command(Command::Split { at_tl: 500_000 });
    /// ```
    Split {
        at_tl: i64,
    },
    /// Cuts the segment at `at_tl` in timeline ticks.
    ///
    /// # Example
    /// ```ignore
    /// use std::path::PathBuf;
    /// use engine::{Command, Engine, FfmpegMediaBackend};
    ///
    /// let mut engine = Engine::new(FfmpegMediaBackend);
    /// let _ = engine.handle_command(Command::Import {
    ///     path: PathBuf::from("demo.mp4"),
    /// });
    /// let _ = engine.handle_command(Command::Cut { at_tl: 500_000 });
    /// ```
    Cut {
        at_tl: i64,
    },
    /// Moves one segment to `new_start_tl` in timeline ticks.
    ///
    /// The engine clamps `new_start_tl` so the moved segment does not overlap
    /// adjacent segments and timeline arithmetic remains valid. Returns
    /// `SegmentIdNotFound` if `segment_id` does not exist.
    ///
    /// # Example
    /// ```ignore
    /// use std::path::PathBuf;
    /// use engine::{Command, Engine, FfmpegMediaBackend};
    ///
    /// let mut engine = Engine::new(FfmpegMediaBackend);
    /// let _ = engine.handle_command(Command::Import {
    ///     path: PathBuf::from("demo.mp4"),
    /// });
    /// let _ = engine.handle_command(Command::MoveSegment {
    ///     segment_id: 7,
    ///     new_start_tl: 900_000,
    /// });
    /// ```
    MoveSegment {
        segment_id: u64,
        new_start_tl: i64,
    },
    /// Trims one segment start to `new_start_tl` in timeline ticks.
    ///
    /// `new_start_tl` is interpreted as the desired inclusive timeline start.
    /// The engine clamps it to preserve segment ordering and keep at least one
    /// timeline tick of duration. Returns `SegmentIdNotFound` when missing.
    ///
    /// # Example
    /// ```ignore
    /// use std::path::PathBuf;
    /// use engine::{Command, Engine, FfmpegMediaBackend};
    ///
    /// let mut engine = Engine::new(FfmpegMediaBackend);
    /// let _ = engine.handle_command(Command::Import {
    ///     path: PathBuf::from("demo.mp4"),
    /// });
    /// let _ = engine.handle_command(Command::TrimSegmentStart {
    ///     segment_id: 7,
    ///     new_start_tl: 400_000,
    /// });
    /// ```
    TrimSegmentStart {
        segment_id: u64,
        new_start_tl: i64,
    },
    /// Trims one segment end to `new_end_tl` in timeline ticks.
    ///
    /// `new_end_tl` is an exclusive timeline boundary. The engine clamps it to
    /// preserve ordering and keep at least one timeline tick of duration.
    /// Returns `SegmentIdNotFound` when `segment_id` does not exist.
    ///
    /// # Example
    /// ```ignore
    /// use std::path::PathBuf;
    /// use engine::{Command, Engine, FfmpegMediaBackend};
    ///
    /// let mut engine = Engine::new(FfmpegMediaBackend);
    /// let _ = engine.handle_command(Command::Import {
    ///     path: PathBuf::from("demo.mp4"),
    /// });
    /// let _ = engine.handle_command(Command::TrimSegmentEnd {
    ///     segment_id: 7,
    ///     new_end_tl: 800_000,
    /// });
    /// ```
    TrimSegmentEnd {
        segment_id: u64,
        new_end_tl: i64,
    },
    Export {
        path: PathBuf,
        settings: ExportSettings,
    },
    CancelExport,
}

/// Events emitted by the engine.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    ProjectChanged(ProjectSnapshot),
    PlayheadChanged { t_tl: i64 },
    PreviewFrameReady { t_tl: i64, frame: PreviewFrame },
    ExportProgress { done: u64, total: u64 },
    ExportFinished { path: PathBuf },
    Error(EngineErrorEvent),
}

/// User-facing error payload emitted as an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineErrorKind {
    SplitPointAtBoundary,
    SegmentNotFound,
    Other,
}

impl From<&EngineError> for EngineErrorKind {
    fn from(value: &EngineError) -> Self {
        match value {
            EngineError::SplitPointAtBoundary { .. } => Self::SplitPointAtBoundary,
            EngineError::SegmentNotFound { .. } => Self::SegmentNotFound,
            EngineError::SegmentIdNotFound { .. } => Self::SegmentNotFound,
            _ => Self::Other,
        }
    }
}

/// User-facing error payload emitted as an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineErrorEvent {
    pub kind: EngineErrorKind,
    pub message: String,
}

impl EngineErrorEvent {
    pub fn from_error(error: &EngineError) -> Self {
        Self {
            kind: EngineErrorKind::from(error),
            message: error.to_string(),
        }
    }
}

/// Export settings for video-only MVP export.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExportSettings {}

/// Immutable project snapshot consumed by the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSnapshot {
    pub assets: Vec<MediaAssetSummary>,
    pub segments: Vec<SegmentSummary>,
    pub duration_tl: i64,
}

/// Snapshot representation of one media asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaAssetSummary {
    pub id: u64,
    pub path: PathBuf,
    pub has_video: bool,
    pub has_audio: bool,
    pub duration_tl: i64,
}

/// Snapshot representation of one timeline segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentSummary {
    pub id: u64,
    pub asset_id: u64,
    pub timeline_start: i64,
    pub timeline_duration: i64,
    pub src_in_video: Option<i64>,
    pub src_out_video: Option<i64>,
    pub src_in_audio: Option<i64>,
    pub src_out_audio: Option<i64>,
}

/// Engine implementation for import/scrub/split/export commands.
#[derive(Debug)]
pub struct Engine<M> {
    media: M,
    project: Option<Project>,
    playhead_tl: i64,
    next_asset_id: u64,
    next_segment_id: u64,
}

impl<M> Engine<M>
where
    M: MediaBackend,
{
    /// Creates a new engine with the provided media backend.
    ///
    /// # Example
    /// ```no_run
    /// use engine::{Engine, FfmpegMediaBackend};
    ///
    /// let _engine = Engine::new(FfmpegMediaBackend);
    /// ```
    pub fn new(media: M) -> Self {
        Self {
            media,
            project: None,
            playhead_tl: 0,
            next_asset_id: 1,
            next_segment_id: 1,
        }
    }

    /// Applies one command and returns emitted events.
    pub fn handle_command(&mut self, command: Command) -> Result<Vec<Event>> {
        match command {
            Command::Import { path } => self.import(path),
            Command::SetPlayhead { t_tl } => self.set_playhead(t_tl),
            Command::Split { at_tl } => self.split(at_tl),
            Command::Cut { at_tl } => self.cut(at_tl),
            Command::MoveSegment {
                segment_id,
                new_start_tl,
            } => self.move_segment(segment_id, new_start_tl),
            Command::TrimSegmentStart {
                segment_id,
                new_start_tl,
            } => self.trim_segment_start(segment_id, new_start_tl),
            Command::TrimSegmentEnd {
                segment_id,
                new_end_tl,
            } => self.trim_segment_end(segment_id, new_end_tl),
            Command::Export { path, settings } => self.export(path, settings),
            Command::CancelExport => Ok(Vec::new()),
        }
    }

    fn import(&mut self, path: PathBuf) -> Result<Vec<Event>> {
        let probed = self.media.probe(&path)?;
        let asset_id = self.allocate_asset_id();
        let segment_id = self.allocate_segment_id();

        let project = Project::from_single_asset(asset_id, segment_id, probed)?;
        let snapshot = project.snapshot();
        self.playhead_tl = 0;
        self.project = Some(project);

        Ok(vec![
            Event::ProjectChanged(snapshot),
            Event::PlayheadChanged { t_tl: 0 },
        ])
    }

    fn set_playhead(&mut self, t_tl: i64) -> Result<Vec<Event>> {
        let project = self.project.as_ref().ok_or(EngineError::ProjectNotLoaded)?;
        let clamped = normalize_playhead(t_tl, project.duration_tl());
        self.playhead_tl = clamped;

        let mut events = vec![Event::PlayheadChanged { t_tl: clamped }];
        let request = match project.preview_request_at(clamped) {
            Ok(request) => Some(request),
            Err(EngineError::SegmentNotFound { .. }) => None,
            Err(error) => return Err(error),
        };
        if let Some(request) = request {
            let frame = self
                .media
                .decode_preview_frame(&request.path, request.source_seconds)?;
            events.push(Event::PreviewFrameReady {
                t_tl: clamped,
                frame,
            });
        }

        Ok(events)
    }

    fn split(&mut self, at_tl: i64) -> Result<Vec<Event>> {
        let next_segment_id = self.next_segment_id;
        {
            let project = self.project.as_mut().ok_or(EngineError::ProjectNotLoaded)?;
            project.split(at_tl, next_segment_id)?;
        }
        let allocated_segment_id = self.allocate_segment_id();
        debug_assert_eq!(
            allocated_segment_id, next_segment_id,
            "allocated segment id diverged from the split request id"
        );
        let project = self.project.as_ref().ok_or(EngineError::ProjectNotLoaded)?;

        info!(
            at_tl,
            next_segment_id,
            segment_count = project.timeline.segments.len(),
            "split applied"
        );

        Ok(vec![Event::ProjectChanged(project.snapshot())])
    }

    fn cut(&mut self, at_tl: i64) -> Result<Vec<Event>> {
        {
            let project = self.project.as_mut().ok_or(EngineError::ProjectNotLoaded)?;
            project.cut(at_tl)?;
        }
        let project = self.project.as_ref().ok_or(EngineError::ProjectNotLoaded)?;
        self.playhead_tl = normalize_playhead(self.playhead_tl, project.duration_tl());

        info!(
            at_tl,
            segment_count = project.timeline.segments.len(),
            playhead_tl = self.playhead_tl,
            "cut applied"
        );

        Ok(vec![Event::ProjectChanged(project.snapshot())])
    }

    fn move_segment(&mut self, segment_id: u64, new_start_tl: i64) -> Result<Vec<Event>> {
        {
            let project = self.project.as_mut().ok_or(EngineError::ProjectNotLoaded)?;
            project.move_segment(segment_id, new_start_tl)?;
        }
        let project = self.project.as_ref().ok_or(EngineError::ProjectNotLoaded)?;
        self.playhead_tl = normalize_playhead(self.playhead_tl, project.duration_tl());
        Ok(vec![Event::ProjectChanged(project.snapshot())])
    }

    fn trim_segment_start(&mut self, segment_id: u64, new_start_tl: i64) -> Result<Vec<Event>> {
        {
            let project = self.project.as_mut().ok_or(EngineError::ProjectNotLoaded)?;
            project.trim_segment_start(segment_id, new_start_tl)?;
        }
        let project = self.project.as_ref().ok_or(EngineError::ProjectNotLoaded)?;
        self.playhead_tl = normalize_playhead(self.playhead_tl, project.duration_tl());
        Ok(vec![Event::ProjectChanged(project.snapshot())])
    }

    fn trim_segment_end(&mut self, segment_id: u64, new_end_tl: i64) -> Result<Vec<Event>> {
        {
            let project = self.project.as_mut().ok_or(EngineError::ProjectNotLoaded)?;
            project.trim_segment_end(segment_id, new_end_tl)?;
        }
        let project = self.project.as_ref().ok_or(EngineError::ProjectNotLoaded)?;
        self.playhead_tl = normalize_playhead(self.playhead_tl, project.duration_tl());
        Ok(vec![Event::ProjectChanged(project.snapshot())])
    }

    fn export(&mut self, path: PathBuf, _settings: ExportSettings) -> Result<Vec<Event>> {
        let project = self.project.as_ref().ok_or(EngineError::ProjectNotLoaded)?;
        let plan = build_video_export_plan(project, path.clone())?;
        let total = plan.segments.len() as u64;

        self.media.export_video(&plan)?;

        Ok(vec![
            Event::ExportProgress { done: total, total },
            Event::ExportFinished { path },
        ])
    }

    fn allocate_asset_id(&mut self) -> u64 {
        let id = self.next_asset_id;
        self.next_asset_id += 1;
        id
    }

    fn allocate_segment_id(&mut self) -> u64 {
        let id = self.next_segment_id;
        self.next_segment_id += 1;
        id
    }
}

impl Engine<FfmpegMediaBackend> {
    /// Creates an engine wired to the FFmpeg backend.
    pub fn with_ffmpeg() -> Self {
        Self::new(FfmpegMediaBackend)
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use super::{Command, Engine, Event, ExportSettings};
    use crate::export::{ExportAudioSettings, ExportVideoPlan, ExportVideoSegment};
    use crate::preview::{
        MediaBackend, PreviewFrame, PreviewPixelFormat, ProbedAudioStream, ProbedMedia,
        ProbedVideoStream,
    };
    use crate::time::{Rational, rescale};

    #[test]
    fn import_creates_single_segment_covering_full_duration() {
        let mut engine = Engine::new(MockBackend::new(sample_probed_media(), sample_frame()));

        let events = engine
            .handle_command(Command::Import {
                path: PathBuf::from("demo.mp4"),
            })
            .expect("import should succeed");

        assert_eq!(events.len(), 2);
        let Event::ProjectChanged(snapshot) = &events[0] else {
            panic!("first event must be ProjectChanged");
        };
        assert_eq!(events[1], Event::PlayheadChanged { t_tl: 0 });

        assert_eq!(snapshot.assets.len(), 1);
        assert_eq!(snapshot.segments.len(), 1);
        assert_eq!(snapshot.duration_tl, 1_200_000);

        let segment = &snapshot.segments[0];
        assert_eq!(segment.timeline_start, 0);
        assert_eq!(segment.timeline_duration, 1_200_000);
        assert_eq!(segment.src_in_video, Some(90_000));
        assert_eq!(segment.src_out_video, Some(198_000));
        assert_eq!(segment.src_in_audio, Some(48_000));
        assert_eq!(segment.src_out_audio, Some(105_600));
    }

    #[test]
    fn set_playhead_emits_preview_frame_ready_from_mapped_source_time() {
        let backend = MockBackend::new(sample_probed_media(), sample_frame());
        let calls = backend.decode_calls();
        let mut engine = Engine::new(backend);
        engine
            .handle_command(Command::Import {
                path: PathBuf::from("demo.mp4"),
            })
            .expect("import should succeed");

        let events = engine
            .handle_command(Command::SetPlayhead { t_tl: 500_000 })
            .expect("set playhead should succeed");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0], Event::PlayheadChanged { t_tl: 500_000 });
        let Event::PreviewFrameReady { t_tl, frame } = &events[1] else {
            panic!("second event must be PreviewFrameReady");
        };
        assert_eq!(*t_tl, 500_000);
        assert_eq!(frame.width, 160);
        assert_eq!(frame.height, 90);

        let decoded_seconds = calls.lock().expect("lock decode calls")[0];
        assert!((decoded_seconds - 1.5).abs() < 1e-6);
    }

    #[test]
    fn split_creates_two_contiguous_segments_with_split_source_ranges() {
        let mut engine = Engine::new(MockBackend::new(sample_probed_media(), sample_frame()));
        engine
            .handle_command(Command::Import {
                path: PathBuf::from("demo.mp4"),
            })
            .expect("import should succeed");

        let events = engine
            .handle_command(Command::Split { at_tl: 333_333 })
            .expect("split should succeed");

        let Event::ProjectChanged(snapshot) = &events[0] else {
            panic!("split must emit ProjectChanged");
        };
        assert_eq!(snapshot.segments.len(), 2);

        let left = &snapshot.segments[0];
        assert_eq!(left.timeline_start, 0);
        assert_eq!(left.timeline_duration, 333_333);
        assert_eq!(left.src_in_video, Some(90_000));
        assert_eq!(left.src_out_video, Some(120_000));
        assert_eq!(left.src_in_audio, Some(48_000));
        assert_eq!(left.src_out_audio, Some(64_000));

        let right = &snapshot.segments[1];
        assert_eq!(right.timeline_start, 333_333);
        assert_eq!(right.timeline_duration, 866_667);
        assert_eq!(right.src_in_video, Some(120_000));
        assert_eq!(right.src_out_video, Some(198_000));
        assert_eq!(right.src_in_audio, Some(64_000));
        assert_eq!(right.src_out_audio, Some(105_600));
    }

    #[test]
    fn split_at_timeline_boundaries_returns_error() {
        let mut engine = Engine::new(MockBackend::new(sample_probed_media(), sample_frame()));
        engine
            .handle_command(Command::Import {
                path: PathBuf::from("demo.mp4"),
            })
            .expect("import should succeed");

        let start = engine.handle_command(Command::Split { at_tl: 0 });
        assert!(matches!(
            start,
            Err(crate::error::EngineError::SplitPointAtBoundary { at_tl: 0 })
        ));

        let end = engine.handle_command(Command::Split { at_tl: 1_200_000 });
        assert!(matches!(
            end,
            Err(crate::error::EngineError::SplitPointAtBoundary { at_tl: 1_200_000 })
        ));
    }

    #[test]
    fn cut_middle_segment_preserves_gap_in_timeline() {
        let mut engine = Engine::new(MockBackend::new(sample_probed_media(), sample_frame()));
        engine
            .handle_command(Command::Import {
                path: PathBuf::from("demo.mp4"),
            })
            .expect("import should succeed");
        engine
            .handle_command(Command::Split { at_tl: 300_000 })
            .expect("first split should succeed");
        engine
            .handle_command(Command::Split { at_tl: 900_000 })
            .expect("second split should succeed");

        let events = engine
            .handle_command(Command::Cut { at_tl: 500_000 })
            .expect("cut should succeed");
        let Event::ProjectChanged(snapshot) = &events[0] else {
            panic!("cut must emit ProjectChanged");
        };

        assert_eq!(snapshot.duration_tl, 1_200_000);
        assert_eq!(snapshot.segments.len(), 2);
        assert_eq!(snapshot.segments[0].timeline_start, 0);
        assert_eq!(snapshot.segments[0].timeline_duration, 300_000);
        assert_eq!(snapshot.segments[0].src_in_video, Some(90_000));
        assert_eq!(snapshot.segments[0].src_out_video, Some(117_000));
        assert_eq!(snapshot.segments[0].src_in_audio, Some(48_000));
        assert_eq!(snapshot.segments[0].src_out_audio, Some(62_400));

        assert_eq!(snapshot.segments[1].timeline_start, 900_000);
        assert_eq!(snapshot.segments[1].timeline_duration, 300_000);
        assert_eq!(snapshot.segments[1].src_in_video, Some(171_000));
        assert_eq!(snapshot.segments[1].src_out_video, Some(198_000));
        assert_eq!(snapshot.segments[1].src_in_audio, Some(91_200));
        assert_eq!(snapshot.segments[1].src_out_audio, Some(105_600));
    }

    #[test]
    fn move_segment_repositions_clip_without_changing_source_range() {
        let mut engine = Engine::new(MockBackend::new(sample_probed_media(), sample_frame()));
        engine
            .handle_command(Command::Import {
                path: PathBuf::from("demo.mp4"),
            })
            .expect("import should succeed");
        engine
            .handle_command(Command::Split { at_tl: 300_000 })
            .expect("first split should succeed");
        engine
            .handle_command(Command::Split { at_tl: 900_000 })
            .expect("second split should succeed");

        let events = engine
            .handle_command(Command::MoveSegment {
                segment_id: 3,
                new_start_tl: 1_000_000,
            })
            .expect("move should succeed");
        let Event::ProjectChanged(snapshot) = &events[0] else {
            panic!("move must emit ProjectChanged");
        };

        assert_eq!(snapshot.duration_tl, 1_300_000);
        assert_eq!(snapshot.segments.len(), 3);

        let moved = &snapshot.segments[2];
        assert_eq!(moved.id, 3);
        assert_eq!(moved.timeline_start, 1_000_000);
        assert_eq!(moved.timeline_duration, 300_000);
        assert_eq!(moved.src_in_video, Some(171_000));
        assert_eq!(moved.src_out_video, Some(198_000));
        assert_eq!(moved.src_in_audio, Some(91_200));
        assert_eq!(moved.src_out_audio, Some(105_600));
    }

    #[test]
    fn trim_segment_start_updates_timeline_and_source_in() {
        let mut engine = Engine::new(MockBackend::new(sample_probed_media(), sample_frame()));
        engine
            .handle_command(Command::Import {
                path: PathBuf::from("demo.mp4"),
            })
            .expect("import should succeed");
        engine
            .handle_command(Command::Split { at_tl: 300_000 })
            .expect("first split should succeed");
        engine
            .handle_command(Command::Split { at_tl: 900_000 })
            .expect("second split should succeed");

        let events = engine
            .handle_command(Command::TrimSegmentStart {
                segment_id: 2,
                new_start_tl: 400_000,
            })
            .expect("trim start should succeed");
        let Event::ProjectChanged(snapshot) = &events[0] else {
            panic!("trim start must emit ProjectChanged");
        };

        let trimmed = &snapshot.segments[1];
        assert_eq!(trimmed.id, 2);
        assert_eq!(trimmed.timeline_start, 400_000);
        assert_eq!(trimmed.timeline_duration, 500_000);
        assert_eq!(trimmed.src_in_video, Some(126_000));
        assert_eq!(trimmed.src_out_video, Some(171_000));
        assert_eq!(trimmed.src_in_audio, Some(67_200));
        assert_eq!(trimmed.src_out_audio, Some(91_200));
    }

    #[test]
    fn trim_segment_end_updates_timeline_and_source_out() {
        let mut engine = Engine::new(MockBackend::new(sample_probed_media(), sample_frame()));
        engine
            .handle_command(Command::Import {
                path: PathBuf::from("demo.mp4"),
            })
            .expect("import should succeed");
        engine
            .handle_command(Command::Split { at_tl: 300_000 })
            .expect("first split should succeed");
        engine
            .handle_command(Command::Split { at_tl: 900_000 })
            .expect("second split should succeed");

        let events = engine
            .handle_command(Command::TrimSegmentEnd {
                segment_id: 2,
                new_end_tl: 800_000,
            })
            .expect("trim end should succeed");
        let Event::ProjectChanged(snapshot) = &events[0] else {
            panic!("trim end must emit ProjectChanged");
        };

        let trimmed = &snapshot.segments[1];
        assert_eq!(trimmed.id, 2);
        assert_eq!(trimmed.timeline_start, 300_000);
        assert_eq!(trimmed.timeline_duration, 500_000);
        assert_eq!(trimmed.src_in_video, Some(117_000));
        assert_eq!(trimmed.src_out_video, Some(162_000));
        assert_eq!(trimmed.src_in_audio, Some(62_400));
        assert_eq!(trimmed.src_out_audio, Some(86_400));
    }

    #[test]
    fn set_playhead_inside_gap_emits_playhead_changed_without_preview_decode() {
        let backend = MockBackend::new(sample_probed_media(), sample_frame());
        let calls = backend.decode_calls();
        let mut engine = Engine::new(backend);
        engine
            .handle_command(Command::Import {
                path: PathBuf::from("demo.mp4"),
            })
            .expect("import should succeed");
        engine
            .handle_command(Command::Split { at_tl: 300_000 })
            .expect("first split should succeed");
        engine
            .handle_command(Command::Split { at_tl: 900_000 })
            .expect("second split should succeed");
        engine
            .handle_command(Command::Cut { at_tl: 500_000 })
            .expect("cut should succeed");

        let events = engine
            .handle_command(Command::SetPlayhead { t_tl: 600_000 })
            .expect("set playhead should succeed");

        assert_eq!(events, vec![Event::PlayheadChanged { t_tl: 600_000 }]);
        assert!(calls.lock().expect("lock decode calls").is_empty());
    }

    #[test]
    fn failed_split_does_not_consume_next_segment_id() {
        let mut engine = Engine::new(MockBackend::new(sample_probed_media(), sample_frame()));
        engine
            .handle_command(Command::Import {
                path: PathBuf::from("demo.mp4"),
            })
            .expect("import should succeed");

        let boundary_result = engine.handle_command(Command::Split { at_tl: 0 });
        assert!(matches!(
            boundary_result,
            Err(crate::error::EngineError::SplitPointAtBoundary { at_tl: 0 })
        ));

        let events = engine
            .handle_command(Command::Split { at_tl: 333_333 })
            .expect("split should succeed");
        let Event::ProjectChanged(snapshot) = &events[0] else {
            panic!("split must emit ProjectChanged");
        };

        let ids: Vec<u64> = snapshot.segments.iter().map(|segment| segment.id).collect();
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn repeated_splits_keep_timeline_contiguous_and_duration_stable() {
        let mut engine = Engine::new(MockBackend::new(sample_probed_media(), sample_frame()));
        engine
            .handle_command(Command::Import {
                path: PathBuf::from("demo.mp4"),
            })
            .expect("import should succeed");
        engine
            .handle_command(Command::Split { at_tl: 333_333 })
            .expect("first split should succeed");

        let events = engine
            .handle_command(Command::Split { at_tl: 900_000 })
            .expect("second split should succeed");
        let Event::ProjectChanged(snapshot) = &events[0] else {
            panic!("split must emit ProjectChanged");
        };

        assert_eq!(snapshot.duration_tl, 1_200_000);
        assert_eq!(snapshot.segments.len(), 3);

        let first = &snapshot.segments[0];
        let second = &snapshot.segments[1];
        let third = &snapshot.segments[2];

        assert_eq!(
            first.timeline_start + first.timeline_duration,
            second.timeline_start
        );
        assert_eq!(
            second.timeline_start + second.timeline_duration,
            third.timeline_start
        );
        assert_eq!(
            third.timeline_start + third.timeline_duration,
            snapshot.duration_tl
        );
    }

    #[test]
    fn import_emits_playhead_reset_after_scrubbing_previous_project() {
        let mut engine = Engine::new(MockBackend::new(sample_probed_media(), sample_frame()));
        engine
            .handle_command(Command::Import {
                path: PathBuf::from("first.mp4"),
            })
            .expect("first import should succeed");
        engine
            .handle_command(Command::SetPlayhead { t_tl: 500_000 })
            .expect("set playhead should succeed");

        let events = engine
            .handle_command(Command::Import {
                path: PathBuf::from("second.mp4"),
            })
            .expect("second import should succeed");

        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], Event::ProjectChanged(_)));
        assert_eq!(events[1], Event::PlayheadChanged { t_tl: 0 });
    }

    #[test]
    fn set_playhead_clamps_negative_mapped_source_time_to_zero_seconds() {
        let backend = MockBackend::new(
            sample_probed_media_with_negative_video_start(),
            sample_frame(),
        );
        let calls = backend.decode_calls();
        let mut engine = Engine::new(backend);
        engine
            .handle_command(Command::Import {
                path: PathBuf::from("demo.mp4"),
            })
            .expect("import should succeed");

        let events = engine
            .handle_command(Command::SetPlayhead { t_tl: 0 })
            .expect("set playhead should succeed");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0], Event::PlayheadChanged { t_tl: 0 });
        let decoded_seconds = calls.lock().expect("lock decode calls")[0];
        assert_eq!(decoded_seconds, 0.0);
    }

    #[test]
    fn export_calls_backend_with_timeline_ordered_segments() {
        let backend = MockBackend::new(sample_probed_media(), sample_frame());
        let export_calls = backend.export_calls();
        let mut engine = Engine::new(backend);
        engine
            .handle_command(Command::Import {
                path: PathBuf::from("demo.mp4"),
            })
            .expect("import should succeed");
        engine
            .handle_command(Command::Split { at_tl: 333_333 })
            .expect("split should succeed");

        let output_path = PathBuf::from("out.mp4");
        let events = engine
            .handle_command(Command::Export {
                path: output_path.clone(),
                settings: ExportSettings::default(),
            })
            .expect("export should succeed");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0], Event::ExportProgress { done: 2, total: 2 });
        assert_eq!(events[1], Event::ExportFinished { path: output_path });

        let calls = export_calls.lock().expect("lock export calls");
        assert_eq!(calls.len(), 1);

        let plan = &calls[0];
        assert_eq!(plan.inputs, vec![PathBuf::from("demo.mp4")]);
        assert_eq!(
            plan.audio,
            Some(ExportAudioSettings {
                sample_rate: 48_000,
                channels: 2,
            })
        );
        assert_eq!(
            plan.segments,
            vec![
                ExportVideoSegment {
                    input_index: 0,
                    src_in_video: 90_000,
                    src_out_video: 120_000,
                    src_video_time_base: Rational::new(1, 90_000).expect("valid rational"),
                    src_in_audio: Some(48_000),
                    src_out_audio: Some(64_000),
                    src_audio_time_base: Some(Rational::new(1, 48_000).expect("valid rational"),),
                },
                ExportVideoSegment {
                    input_index: 0,
                    src_in_video: 120_000,
                    src_out_video: 198_000,
                    src_video_time_base: Rational::new(1, 90_000).expect("valid rational"),
                    src_in_audio: Some(64_000),
                    src_out_audio: Some(105_600),
                    src_audio_time_base: Some(Rational::new(1, 48_000).expect("valid rational"),),
                },
            ]
        );
    }

    #[test]
    fn export_skips_zero_length_video_ranges_created_by_subframe_split() {
        let backend = MockBackend::new(sample_probed_media(), sample_frame());
        let export_calls = backend.export_calls();
        let mut engine = Engine::new(backend);
        engine
            .handle_command(Command::Import {
                path: PathBuf::from("demo.mp4"),
            })
            .expect("import should succeed");
        engine
            .handle_command(Command::Split { at_tl: 1 })
            .expect("split should succeed");

        let events = engine
            .handle_command(Command::Export {
                path: PathBuf::from("out.mp4"),
                settings: ExportSettings::default(),
            })
            .expect("export should succeed");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0], Event::ExportProgress { done: 1, total: 1 });

        let calls = export_calls.lock().expect("lock export calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].segments.len(), 1);
        assert_eq!(
            calls[0].audio,
            Some(ExportAudioSettings {
                sample_rate: 48_000,
                channels: 2,
            })
        );
        assert_eq!(calls[0].segments[0].src_in_video, 90_000);
        assert_eq!(calls[0].segments[0].src_out_video, 198_000);
        assert_eq!(calls[0].segments[0].src_in_audio, Some(48_000));
        assert_eq!(calls[0].segments[0].src_out_audio, Some(105_600));
    }

    #[test]
    fn export_allows_subframe_split_with_zero_length_audio_range() {
        let backend = MockBackend::new(sample_probed_media(), sample_frame());
        let export_calls = backend.export_calls();
        let mut engine = Engine::new(backend);
        engine
            .handle_command(Command::Import {
                path: PathBuf::from("demo.mp4"),
            })
            .expect("import should succeed");
        engine
            .handle_command(Command::Split { at_tl: 8 })
            .expect("split should succeed");

        let events = engine
            .handle_command(Command::Export {
                path: PathBuf::from("out.mp4"),
                settings: ExportSettings::default(),
            })
            .expect("export should succeed");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0], Event::ExportProgress { done: 2, total: 2 });

        let calls = export_calls.lock().expect("lock export calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].segments.len(), 2);
        assert_eq!(calls[0].segments[0].src_in_video, 90_000);
        assert_eq!(calls[0].segments[0].src_out_video, 90_001);
        assert_eq!(calls[0].segments[0].src_in_audio, Some(47_999));
        assert_eq!(calls[0].segments[0].src_out_audio, Some(48_000));
    }

    #[test]
    fn export_subframe_split_near_audio_end_keeps_audio_range_in_bounds() {
        let backend = MockBackend::new(sample_probed_media(), sample_frame());
        let export_calls = backend.export_calls();
        let mut engine = Engine::new(backend);
        engine
            .handle_command(Command::Import {
                path: PathBuf::from("demo.mp4"),
            })
            .expect("import should succeed");
        engine
            .handle_command(Command::Split { at_tl: 1_199_990 })
            .expect("split should succeed");

        engine
            .handle_command(Command::Export {
                path: PathBuf::from("out.mp4"),
                settings: ExportSettings::default(),
            })
            .expect("export should succeed");

        let calls = export_calls.lock().expect("lock export calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].segments.len(), 2);

        let right = &calls[0].segments[1];
        assert_eq!(right.src_in_video, 197_999);
        assert_eq!(right.src_out_video, 198_000);
        assert_eq!(right.src_in_audio, Some(105_599));
        assert_eq!(right.src_out_audio, Some(105_600));
    }

    fn sample_probed_media() -> ProbedMedia {
        let duration_tl = 1_200_000;
        let video_tb = Rational::new(1, 90_000).expect("valid rational");
        let audio_tb = Rational::new(1, 48_000).expect("valid rational");

        let video_src_in = 90_000;
        let video_src_out = video_src_in + rescale(duration_tl, Rational::MICROS, video_tb);
        let audio_src_in = 48_000;
        let audio_src_out = audio_src_in + rescale(duration_tl, Rational::MICROS, audio_tb);

        ProbedMedia {
            path: PathBuf::from("demo.mp4"),
            duration_tl,
            video: Some(ProbedVideoStream {
                stream_index: 0,
                time_base: video_tb,
                src_in: video_src_in,
                src_out: video_src_out,
                width: 160,
                height: 90,
            }),
            audio: Some(ProbedAudioStream {
                stream_index: 1,
                time_base: audio_tb,
                src_in: audio_src_in,
                src_out: audio_src_out,
                sample_rate: 48_000,
                channels: 2,
            }),
        }
    }

    fn sample_frame() -> PreviewFrame {
        PreviewFrame {
            width: 160,
            height: 90,
            format: PreviewPixelFormat::Rgba8,
            bytes: Arc::from(vec![0; 160 * 90 * 4]),
        }
    }

    fn sample_probed_media_with_negative_video_start() -> ProbedMedia {
        let mut media = sample_probed_media();
        let video = media.video.as_mut().expect("video stream exists");
        video.src_in = -9_000;
        video.src_out =
            video.src_in + rescale(media.duration_tl, Rational::MICROS, video.time_base);
        media
    }

    #[derive(Debug)]
    struct MockBackend {
        probe: ProbedMedia,
        frame: PreviewFrame,
        decode_calls: Arc<Mutex<Vec<f64>>>,
        export_calls: Arc<Mutex<Vec<ExportVideoPlan>>>,
    }

    impl MockBackend {
        fn new(probe: ProbedMedia, frame: PreviewFrame) -> Self {
            Self {
                probe,
                frame,
                decode_calls: Arc::new(Mutex::new(Vec::new())),
                export_calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn decode_calls(&self) -> Arc<Mutex<Vec<f64>>> {
            Arc::clone(&self.decode_calls)
        }

        fn export_calls(&self) -> Arc<Mutex<Vec<ExportVideoPlan>>> {
            Arc::clone(&self.export_calls)
        }
    }

    impl MediaBackend for MockBackend {
        fn probe(&self, _path: &Path) -> crate::Result<ProbedMedia> {
            Ok(self.probe.clone())
        }

        fn decode_preview_frame(
            &self,
            _path: &Path,
            at_seconds: f64,
        ) -> crate::Result<PreviewFrame> {
            self.decode_calls
                .lock()
                .expect("lock decode calls")
                .push(at_seconds);
            Ok(self.frame.clone())
        }

        fn export_video(&self, plan: &ExportVideoPlan) -> crate::Result<()> {
            self.export_calls
                .lock()
                .expect("lock export calls")
                .push(plan.clone());
            Ok(())
        }
    }
}
