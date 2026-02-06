use std::path::PathBuf;

use crate::error::{EngineError, Result};
use crate::project::Project;
use crate::time::Rational;

/// Export plan for MP4 rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportVideoPlan {
    pub inputs: Vec<PathBuf>,
    pub segments: Vec<ExportVideoSegment>,
    pub audio: Option<ExportAudioSettings>,
    pub output_path: PathBuf,
}

/// Audio output settings used by export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportAudioSettings {
    pub sample_rate: u32,
    pub channels: u16,
}

/// One segment in export order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportVideoSegment {
    pub input_index: usize,
    pub src_in_video: i64,
    pub src_out_video: i64,
    pub src_video_time_base: Rational,
    pub src_in_audio: Option<i64>,
    pub src_out_audio: Option<i64>,
    pub src_audio_time_base: Option<Rational>,
}

/// Builds an export plan from the current project timeline.
pub fn build_video_export_plan(project: &Project, output_path: PathBuf) -> Result<ExportVideoPlan> {
    let mut inputs = Vec::<PathBuf>::new();
    let mut segments = Vec::<ExportVideoSegment>::new();
    let mut selected = Vec::<(&crate::timeline::Segment, &crate::project::MediaAsset)>::new();

    for timeline_segment in &project.timeline.segments {
        let asset = project
            .assets
            .iter()
            .find(|asset| asset.id == timeline_segment.asset_id)
            .ok_or(EngineError::MissingAsset {
                asset_id: timeline_segment.asset_id,
            })?;
        let video = asset
            .video
            .ok_or(EngineError::MissingVideoStream { asset_id: asset.id })?;
        let src_in_video = timeline_segment
            .src_in_video
            .ok_or(EngineError::MissingVideoRange {
                segment_id: timeline_segment.id,
            })?;
        let src_out_video =
            timeline_segment
                .src_out_video
                .ok_or(EngineError::MissingVideoRange {
                    segment_id: timeline_segment.id,
                })?;

        if src_out_video < src_in_video {
            return Err(EngineError::InvalidVideoRange {
                segment_id: timeline_segment.id,
                src_in_video,
                src_out_video,
            });
        }
        if src_out_video == src_in_video {
            continue;
        }

        let input_index = if let Some(index) = inputs.iter().position(|path| *path == asset.path) {
            index
        } else {
            let index = inputs.len();
            inputs.push(asset.path.clone());
            index
        };

        segments.push(ExportVideoSegment {
            input_index,
            src_in_video,
            src_out_video,
            src_video_time_base: video.time_base,
            src_in_audio: None,
            src_out_audio: None,
            src_audio_time_base: None,
        });
        selected.push((timeline_segment, asset));
    }

    let has_audio = selected.iter().any(|(_, asset)| asset.audio.is_some());
    let mut audio = None;
    if has_audio {
        let first_audio = selected.iter().find_map(|(_, asset)| asset.audio).ok_or(
            EngineError::MissingAudioStream {
                asset_id: selected
                    .first()
                    .map(|(_, asset)| asset.id)
                    .unwrap_or_default(),
            },
        )?;
        audio = Some(ExportAudioSettings {
            sample_rate: first_audio.sample_rate,
            channels: first_audio.channels,
        });

        for (index, (timeline_segment, asset)) in selected.iter().enumerate() {
            let audio_stream = asset
                .audio
                .ok_or(EngineError::MissingAudioStream { asset_id: asset.id })?;
            let src_in_audio =
                timeline_segment
                    .src_in_audio
                    .ok_or(EngineError::MissingAudioRange {
                        segment_id: timeline_segment.id,
                    })?;
            let src_out_audio =
                timeline_segment
                    .src_out_audio
                    .ok_or(EngineError::MissingAudioRange {
                        segment_id: timeline_segment.id,
                    })?;
            if src_out_audio < src_in_audio {
                return Err(EngineError::InvalidAudioRange {
                    segment_id: timeline_segment.id,
                    src_in_audio,
                    src_out_audio,
                });
            }

            // Fine-grained splits can round to zero audio ticks while video stays positive.
            // Keep the project exportable by extending to one audio tick.
            let src_out_audio = if src_out_audio == src_in_audio {
                src_out_audio.saturating_add(1)
            } else {
                src_out_audio
            };
            let export_segment = &mut segments[index];
            export_segment.src_in_audio = Some(src_in_audio);
            export_segment.src_out_audio = Some(src_out_audio);
            export_segment.src_audio_time_base = Some(audio_stream.time_base);
        }
    }

    Ok(ExportVideoPlan {
        inputs,
        segments,
        audio,
        output_path,
    })
}
