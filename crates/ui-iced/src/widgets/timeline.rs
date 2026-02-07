use engine::ProjectSnapshot;

/// Rect-like representation of one timeline segment for drawing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentStrip {
    pub segment_id: u64,
    pub x: f32,
    pub width: f32,
}

/// Values needed by the UI to draw timeline segments and playhead.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineRenderModel {
    pub segments: Vec<SegmentStrip>,
    pub playhead_x: f32,
}

/// Interaction result emitted by the timeline widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineInteraction {
    Scrubbed(i64),
    SplitRequested,
}

/// Builds draw data for a timeline widget.
pub fn build_render_model(
    snapshot: &ProjectSnapshot,
    playhead_tl: i64,
    width_px: f32,
) -> TimelineRenderModel {
    let safe_width = width_px.max(0.0);
    let duration_tl = snapshot.duration_tl.max(1);
    let scale = safe_width / duration_tl as f32;

    let segments = snapshot
        .segments
        .iter()
        .map(|segment| SegmentStrip {
            segment_id: segment.id,
            x: segment.timeline_start as f32 * scale,
            width: segment.timeline_duration.max(0) as f32 * scale,
        })
        .collect();

    let clamped_playhead = playhead_tl.clamp(0, duration_tl - 1);
    TimelineRenderModel {
        segments,
        playhead_x: clamped_playhead as f32 * scale,
    }
}

/// Maps a pointer X position into timeline ticks for scrubbing.
pub fn scrub_timeline_at_x(x_px: f32, width_px: f32, duration_tl: i64) -> i64 {
    if duration_tl <= 0 {
        return 0;
    }
    if width_px <= 0.0 {
        return 0;
    }

    let normalized = (x_px / width_px).clamp(0.0, 1.0);
    let max_tick = duration_tl - 1;
    (normalized * max_tick as f32).round() as i64
}

/// Creates a scrub interaction from a click on the timeline.
pub fn click_at_x(x_px: f32, width_px: f32, duration_tl: i64) -> TimelineInteraction {
    TimelineInteraction::Scrubbed(scrub_timeline_at_x(x_px, width_px, duration_tl))
}

/// Creates a scrub interaction from a drag update on the timeline.
pub fn drag_to_x(x_px: f32, width_px: f32, duration_tl: i64) -> TimelineInteraction {
    TimelineInteraction::Scrubbed(scrub_timeline_at_x(x_px, width_px, duration_tl))
}

/// Creates a split interaction (for keyboard shortcut or dedicated button).
pub fn request_split() -> TimelineInteraction {
    TimelineInteraction::SplitRequested
}

#[cfg(test)]
mod tests {
    use engine::ProjectSnapshot;
    use engine::api::SegmentSummary;

    use super::{
        TimelineInteraction, build_render_model, click_at_x, drag_to_x, request_split,
        scrub_timeline_at_x,
    };

    fn sample_snapshot() -> ProjectSnapshot {
        ProjectSnapshot {
            assets: Vec::new(),
            segments: vec![
                SegmentSummary {
                    id: 10,
                    asset_id: 1,
                    timeline_start: 0,
                    timeline_duration: 600,
                    src_in_video: None,
                    src_out_video: None,
                    src_in_audio: None,
                    src_out_audio: None,
                },
                SegmentSummary {
                    id: 11,
                    asset_id: 1,
                    timeline_start: 600,
                    timeline_duration: 400,
                    src_in_video: None,
                    src_out_video: None,
                    src_in_audio: None,
                    src_out_audio: None,
                },
            ],
            duration_tl: 1_000,
        }
    }

    #[test]
    fn build_render_model_positions_segments_and_playhead() {
        let snapshot = sample_snapshot();
        let model = build_render_model(&snapshot, 250, 100.0);

        assert_eq!(model.segments.len(), 2);
        assert_eq!(model.segments[0].x, 0.0);
        assert_eq!(model.segments[0].width, 60.0);
        assert_eq!(model.segments[1].x, 60.0);
        assert_eq!(model.segments[1].width, 40.0);
        assert_eq!(model.playhead_x, 25.0);
    }

    #[test]
    fn scrub_position_is_clamped_and_scaled() {
        assert_eq!(scrub_timeline_at_x(-10.0, 200.0, 1_000), 0);
        assert_eq!(scrub_timeline_at_x(100.0, 200.0, 1_000), 500);
        assert_eq!(scrub_timeline_at_x(220.0, 200.0, 1_000), 999);
    }

    #[test]
    fn click_and_drag_emit_scrub_interactions() {
        assert_eq!(
            click_at_x(50.0, 200.0, 1_000),
            TimelineInteraction::Scrubbed(250)
        );
        assert_eq!(
            drag_to_x(75.0, 100.0, 400),
            TimelineInteraction::Scrubbed(299)
        );
    }

    #[test]
    fn split_request_emits_split_interaction() {
        assert_eq!(request_split(), TimelineInteraction::SplitRequested);
    }
}
