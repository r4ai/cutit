use crate::error::{EngineError, Result};
use crate::time::{Rational, TIMELINE_TIME_BASE, rescale};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// Opaque identifier for timeline segments.
pub type SegmentId = u64;
/// Opaque identifier for media assets.
pub type AssetId = u64;

/// Single-track timeline used in the MVP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timeline {
    pub segments: Vec<Segment>,
}

/// A linear segment referencing one source asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    ///
    /// Returns an error when `at_tl` points to a segment boundary or does not
    /// belong to any segment.
    ///
    /// # Example
    /// ```ignore
    /// use engine::{Rational, timeline::{Segment, Timeline}};
    ///
    /// let mut timeline = Timeline {
    ///     segments: vec![Segment {
    ///         id: 1,
    ///         asset_id: 7,
    ///         src_in_video: Some(0),
    ///         src_out_video: Some(90_000),
    ///         src_in_audio: None,
    ///         src_out_audio: None,
    ///         timeline_start: 0,
    ///         timeline_duration: 1_000_000,
    ///     }],
    /// };
    ///
    /// timeline
    ///     .split_segment(500_000, 2, Some(Rational::new(1, 90_000).unwrap()), None)
    ///     .unwrap();
    /// assert_eq!(timeline.segments.len(), 2);
    /// ```
    pub fn split_segment(
        &mut self,
        at_tl: i64,
        next_segment_id: SegmentId,
        video_time_base: Option<Rational>,
        audio_time_base: Option<Rational>,
    ) -> Result<()> {
        if self.is_boundary_split_point(at_tl) {
            warn!(at_tl, "split rejected: boundary point");
            return Err(EngineError::SplitPointAtBoundary { at_tl });
        }

        let Some(index) = self.find_segment_index(at_tl) else {
            warn!(at_tl, "split rejected: segment not found");
            return Err(EngineError::SegmentNotFound { at_tl });
        };
        let current = self.segments[index].clone();

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

        debug!(
            at_tl,
            segment_id = current.id,
            asset_id = current.asset_id,
            next_segment_id,
            local_tl,
            left_duration,
            right_duration,
            left_video_out = ?left_video_out,
            right_video_in = ?right_video_in,
            left_audio_out = ?left_audio_out,
            right_audio_in = ?right_audio_in,
            "split accepted"
        );

        self.segments[index] = left;
        self.segments.insert(index + 1, right);
        Ok(())
    }

    /// Cuts one segment at timeline timestamp `at_tl`.
    ///
    /// When `at_tl` is exactly a segment start boundary, the segment starting
    /// at that boundary is removed. Otherwise, the segment containing `at_tl`
    /// is removed. Following segments are shifted left to keep the timeline
    /// contiguous.
    ///
    /// # Example
    /// ```ignore
    /// use engine::timeline::{Segment, Timeline};
    ///
    /// let mut timeline = Timeline {
    ///     segments: vec![
    ///         Segment {
    ///             id: 1,
    ///             asset_id: 7,
    ///             src_in_video: Some(0),
    ///             src_out_video: Some(90_000),
    ///             src_in_audio: None,
    ///             src_out_audio: None,
    ///             timeline_start: 0,
    ///             timeline_duration: 1_000_000,
    ///         },
    ///         Segment {
    ///             id: 2,
    ///             asset_id: 7,
    ///             src_in_video: Some(90_000),
    ///             src_out_video: Some(180_000),
    ///             src_in_audio: None,
    ///             src_out_audio: None,
    ///             timeline_start: 1_000_000,
    ///             timeline_duration: 1_000_000,
    ///         },
    ///     ],
    /// };
    ///
    /// let removed = timeline.cut_segment(1_000_000).unwrap();
    /// assert_eq!(removed.id, 2);
    /// assert_eq!(timeline.duration_tl(), 1_000_000);
    /// ```
    pub fn cut_segment(&mut self, at_tl: i64) -> Result<Segment> {
        let index = self
            .find_cut_segment_index(at_tl)
            .ok_or(EngineError::SegmentNotFound { at_tl })?;
        let removed = self.segments.remove(index);
        let removed_duration = removed.timeline_duration;

        for segment in self.segments.iter_mut().skip(index) {
            segment.timeline_start -= removed_duration;
        }

        debug!(
            at_tl,
            removed_segment_id = removed.id,
            removed_duration,
            segment_count = self.segments.len(),
            "cut accepted"
        );

        Ok(removed)
    }

    pub(crate) fn is_boundary_split_point(&self, at_tl: i64) -> bool {
        self.segments.iter().any(|segment| {
            let end = segment.timeline_start + segment.timeline_duration;
            at_tl == segment.timeline_start || at_tl == end
        })
    }

    fn find_cut_segment_index(&self, at_tl: i64) -> Option<usize> {
        self.segments
            .iter()
            .position(|segment| segment.timeline_start == at_tl)
            .or_else(|| self.find_segment_index(at_tl))
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

#[cfg(test)]
mod tests {
    use super::{Segment, Timeline};
    use crate::error::EngineError;

    #[test]
    fn split_at_timeline_end_is_reported_as_boundary() {
        let mut timeline = Timeline {
            segments: vec![Segment {
                id: 1,
                asset_id: 1,
                src_in_video: Some(0),
                src_out_video: Some(100),
                src_in_audio: None,
                src_out_audio: None,
                timeline_start: 0,
                timeline_duration: 1_000,
            }],
        };

        let result = timeline.split_segment(1_000, 2, None, None);
        assert!(matches!(
            result,
            Err(EngineError::SplitPointAtBoundary { at_tl: 1_000 })
        ));
    }

    #[test]
    fn cut_at_boundary_removes_segment_starting_at_boundary() {
        let mut timeline = Timeline {
            segments: vec![
                Segment {
                    id: 1,
                    asset_id: 1,
                    src_in_video: Some(0),
                    src_out_video: Some(10),
                    src_in_audio: None,
                    src_out_audio: None,
                    timeline_start: 0,
                    timeline_duration: 100,
                },
                Segment {
                    id: 2,
                    asset_id: 1,
                    src_in_video: Some(10),
                    src_out_video: Some(20),
                    src_in_audio: None,
                    src_out_audio: None,
                    timeline_start: 100,
                    timeline_duration: 100,
                },
            ],
        };

        let removed = timeline.cut_segment(100).expect("cut should succeed");
        assert_eq!(removed.id, 2);
        assert_eq!(timeline.segments.len(), 1);
        assert_eq!(timeline.duration_tl(), 100);
        assert_eq!(timeline.segments[0].timeline_start, 0);
    }

    #[test]
    fn cut_middle_segment_shifts_following_segments_left() {
        let mut timeline = Timeline {
            segments: vec![
                Segment {
                    id: 1,
                    asset_id: 1,
                    src_in_video: Some(0),
                    src_out_video: Some(10),
                    src_in_audio: None,
                    src_out_audio: None,
                    timeline_start: 0,
                    timeline_duration: 100,
                },
                Segment {
                    id: 2,
                    asset_id: 1,
                    src_in_video: Some(10),
                    src_out_video: Some(20),
                    src_in_audio: None,
                    src_out_audio: None,
                    timeline_start: 100,
                    timeline_duration: 100,
                },
                Segment {
                    id: 3,
                    asset_id: 1,
                    src_in_video: Some(20),
                    src_out_video: Some(30),
                    src_in_audio: None,
                    src_out_audio: None,
                    timeline_start: 200,
                    timeline_duration: 100,
                },
            ],
        };

        let removed = timeline.cut_segment(150).expect("cut should succeed");
        assert_eq!(removed.id, 2);
        assert_eq!(timeline.segments.len(), 2);
        assert_eq!(timeline.segments[1].id, 3);
        assert_eq!(timeline.segments[1].timeline_start, 100);
        assert_eq!(timeline.duration_tl(), 200);
    }
}
