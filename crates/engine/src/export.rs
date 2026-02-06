use std::path::PathBuf;

use crate::error::{EngineError, Result};
use crate::project::Project;
use crate::time::Rational;

/// Export plan for video-only rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportVideoPlan {
    pub inputs: Vec<PathBuf>,
    pub segments: Vec<ExportVideoSegment>,
    pub output_path: PathBuf,
}

/// One video segment in export order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportVideoSegment {
    pub input_index: usize,
    pub src_in_video: i64,
    pub src_out_video: i64,
    pub src_time_base: Rational,
}

/// Builds a video-only export plan from the current project timeline.
pub fn build_video_export_plan(project: &Project, output_path: PathBuf) -> Result<ExportVideoPlan> {
    let mut inputs = Vec::<PathBuf>::new();
    let mut segments = Vec::<ExportVideoSegment>::new();

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
            src_time_base: video.time_base,
        });
    }

    Ok(ExportVideoPlan {
        inputs,
        segments,
        output_path,
    })
}
