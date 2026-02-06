use crate::error::{EngineError, Result};
use crate::time::{Rational, TIMELINE_TIME_BASE, rescale};

/// Opaque identifier for timeline segments.
pub type SegmentId = u64;
/// Opaque identifier for media assets.
pub type AssetId = u64;

/// Single-track timeline used in the MVP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    pub segments: Vec<Segment>,
}

/// A linear segment referencing one source asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub id: SegmentId,
    pub asset_id: AssetId,
    pub src_in_video: Option<i64>,
    pub src_out_video: Option<i64>,
    pub src_in_audio: Option<i64>,
    pub src_out_audio: Option<i64>,
    pub timeline_start: i64,
    pub timeline_duration: i64,
}

impl Timeline {
    /// Returns total timeline duration in timeline ticks.
    pub fn duration_tl(&self) -> i64 {
        self.segments
            .last()
            .map(|segment| segment.timeline_start + segment.timeline_duration)
            .unwrap_or(0)
    }

    /// Finds the segment index that contains `t_tl`.
    pub fn find_segment_index(&self, t_tl: i64) -> Option<usize> {
        self.segments.iter().position(|segment| {
            let end = segment.timeline_start + segment.timeline_duration;
            segment.timeline_start <= t_tl && t_tl < end
        })
    }

    /// Splits one segment into two at timeline timestamp `at_tl`.
    pub fn split_segment(
        &mut self,
        at_tl: i64,
        next_segment_id: SegmentId,
        video_time_base: Option<Rational>,
        audio_time_base: Option<Rational>,
    ) -> Result<()> {
        let index = self
            .find_segment_index(at_tl)
            .ok_or(EngineError::SegmentNotFound { at_tl })?;
        let current = self.segments[index].clone();

        if at_tl == current.timeline_start
            || at_tl == current.timeline_start + current.timeline_duration
        {
            return Err(EngineError::SplitPointAtBoundary { at_tl });
        }

        let local_tl = at_tl - current.timeline_start;
        let left_duration = local_tl;
        let right_duration = current.timeline_duration - local_tl;

        let (left_video_out, right_video_in) = split_stream_range(
            current.src_in_video,
            current.src_out_video,
            left_duration,
            video_time_base,
        );
        let (left_audio_out, right_audio_in) = split_stream_range(
            current.src_in_audio,
            current.src_out_audio,
            left_duration,
            audio_time_base,
        );

        let left = Segment {
            timeline_duration: left_duration,
            src_out_video: left_video_out,
            src_out_audio: left_audio_out,
            ..current.clone()
        };

        let right = Segment {
            id: next_segment_id,
            src_in_video: right_video_in,
            src_in_audio: right_audio_in,
            timeline_start: at_tl,
            timeline_duration: right_duration,
            ..current
        };

        self.segments[index] = left;
        self.segments.insert(index + 1, right);
        Ok(())
    }
}

fn split_stream_range(
    src_in: Option<i64>,
    src_out: Option<i64>,
    left_duration_tl: i64,
    time_base: Option<Rational>,
) -> (Option<i64>, Option<i64>) {
    let (Some(src_in), Some(src_out), Some(time_base)) = (src_in, src_out, time_base) else {
        return (src_out, src_in);
    };

    let delta = rescale(left_duration_tl, TIMELINE_TIME_BASE, time_base);
    let split = (src_in + delta).clamp(src_in, src_out);
    (Some(split), Some(split))
}
