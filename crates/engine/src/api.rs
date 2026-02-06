use std::path::PathBuf;

use crate::error::{EngineError, Result};
use crate::preview::{FfmpegMediaBackend, MediaBackend, PreviewFrame};
use crate::project::{Project, normalize_playhead};

/// Commands accepted by the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Import {
        path: PathBuf,
    },
    SetPlayhead {
        t_tl: i64,
    },
    Split {
        at_tl: i64,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineErrorEvent {
    pub message: String,
}

/// Export settings placeholder for Step 3+.
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

/// Step 2 engine implementation.
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
            Command::Export { .. } | Command::CancelExport => {
                Err(EngineError::ExportNotImplemented)
            }
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

        let request = project.preview_request_at(clamped)?;
        let frame = self
            .media
            .decode_preview_frame(&request.path, request.source_seconds)?;

        Ok(vec![
            Event::PlayheadChanged { t_tl: clamped },
            Event::PreviewFrameReady {
                t_tl: clamped,
                frame,
            },
        ])
    }

    fn split(&mut self, at_tl: i64) -> Result<Vec<Event>> {
        let next_segment_id = self.allocate_segment_id();
        let project = self.project.as_mut().ok_or(EngineError::ProjectNotLoaded)?;
        project.split(at_tl, next_segment_id)?;

        Ok(vec![Event::ProjectChanged(project.snapshot())])
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

    use super::{Command, Engine, Event};
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
                time_base: video_tb,
                src_in: video_src_in,
                src_out: video_src_out,
                width: 160,
                height: 90,
            }),
            audio: Some(ProbedAudioStream {
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
    }

    impl MockBackend {
        fn new(probe: ProbedMedia, frame: PreviewFrame) -> Self {
            Self {
                probe,
                frame,
                decode_calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn decode_calls(&self) -> Arc<Mutex<Vec<f64>>> {
            Arc::clone(&self.decode_calls)
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
    }
}
