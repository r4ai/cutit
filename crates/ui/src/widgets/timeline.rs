use engine::ProjectSnapshot;
use engine::api::SegmentSummary;
use iced::widget::canvas::{self, Path, Stroke};
use iced::widget::container;
use iced::{Color, Element, Length, Point, Rectangle, Size, Theme, mouse};

/// Converts an x coordinate in timeline widget space to a timeline tick.
///
/// The mapping is proportional across the width of the widget, with the left
/// edge corresponding to tick `0` and the right edge corresponding to the
/// last tick (`duration_tl - 1`). Positions outside the widget are clamped.
///
/// # Example
///
/// ```ignore
/// assert_eq!(tick_from_x(0.0, 200.0, 1_000), 0);
/// assert_eq!(tick_from_x(200.0, 200.0, 1_000), 999);
/// assert_eq!(tick_from_x(250.0, 200.0, 1_000), 999);
/// ```
pub fn tick_from_x(x: f32, width: f32, duration_tl: i64) -> i64 {
    if duration_tl <= 0 || width <= 0.0 {
        return 0;
    }

    let clamped_x = x.clamp(0.0, width);
    let ratio = (clamped_x / width) as f64;
    let tick = (ratio * duration_tl as f64).floor() as i64;

    tick.clamp(0, duration_tl - 1)
}

#[derive(Debug, Default)]
struct TimelineState {
    drag_mode: Option<DragMode>,
    drag_start_x: Option<f32>,
}

#[derive(Debug, Clone, Copy)]
enum DragMode {
    Scrub,
    Move {
        segment_id: u64,
        grab_offset_tl: i64,
    },
    TrimStart {
        segment_id: u64,
    },
    TrimEnd {
        segment_id: u64,
    },
}

const DRAG_START_THRESHOLD_PX: f32 = 4.0;
const SEGMENT_VERTICAL_PADDING_PX: f32 = 10.0;

#[derive(Debug)]
struct TimelineProgram<'a, Message> {
    duration_tl: i64,
    playhead_tl: i64,
    split_feedback_tl: Option<i64>,
    segments: &'a [SegmentSummary],
    cache: &'a canvas::Cache,
    on_scrub: fn(i64) -> Message,
    on_split: fn(i64) -> Message,
    on_cut: fn(i64) -> Message,
    on_move: fn(u64, i64) -> Message,
    on_trim_start: fn(u64, i64) -> Message,
    on_trim_end: fn(u64, i64) -> Message,
}

fn playhead_x_from_tick(playhead_tl: i64, duration_tl: i64, width: f32) -> f32 {
    if duration_tl <= 0 {
        return 0.0;
    }

    let max_tick = duration_tl - 1;
    let clamped_tick = playhead_tl.clamp(0, max_tick);
    if clamped_tick == max_tick {
        return width;
    }

    (clamped_tick as f32 / duration_tl as f32) * width
}

fn split_boundary_ticks(segments: &[SegmentSummary], duration_tl: i64) -> Vec<i64> {
    if duration_tl <= 0 {
        return Vec::new();
    }

    let mut ticks = Vec::new();
    for segment in segments {
        let tick = segment.timeline_start;
        if tick <= 0 || tick >= duration_tl {
            continue;
        }

        if ticks.last().copied() != Some(tick) {
            ticks.push(tick);
        }
    }

    ticks
}

fn segment_at_tick(segments: &[SegmentSummary], t_tl: i64) -> Option<&SegmentSummary> {
    segments.iter().find(|segment| {
        let end = segment.timeline_start + segment.timeline_duration;
        segment.timeline_start <= t_tl && t_tl < end
    })
}

fn edge_x_from_tl(t_tl: i64, duration_tl: i64, width: f32) -> f32 {
    if duration_tl <= 0 {
        return 0.0;
    }
    let clamped_tl = t_tl.clamp(0, duration_tl);
    (clamped_tl as f32 / duration_tl as f32) * width
}

fn is_over_segment_layer(y: Option<f32>, height: f32) -> bool {
    let Some(y) = y else {
        return false;
    };

    let top = SEGMENT_VERTICAL_PADDING_PX;
    let bottom = (height - SEGMENT_VERTICAL_PADDING_PX).max(top);
    top <= y && y <= bottom
}

impl<Message> canvas::Program<Message> for TimelineProgram<'_, Message> {
    type State = TimelineState;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        if self.duration_tl <= 0 {
            return (canvas::event::Status::Ignored, None);
        }

        let cursor_x = cursor.position().map(|position| position.x - bounds.x);
        let cursor_y = cursor.position().map(|position| position.y - bounds.y);
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if !cursor.is_over(bounds) {
                    return (canvas::event::Status::Ignored, None);
                }
                let Some(x) = cursor_x else {
                    return (canvas::event::Status::Ignored, None);
                };
                state.drag_start_x = Some(x);
                let tick = tick_from_x(x, bounds.width, self.duration_tl);
                let edge_threshold_px = 6.0;
                let is_over_segment_layer = is_over_segment_layer(cursor_y, bounds.height);

                let segment = if is_over_segment_layer {
                    segment_at_tick(self.segments, tick)
                        .or_else(|| segment_at_tick(self.segments, tick.saturating_sub(1)))
                } else {
                    None
                };
                if let Some(segment) = segment {
                    let start_x =
                        edge_x_from_tl(segment.timeline_start, self.duration_tl, bounds.width);
                    let end_x = edge_x_from_tl(
                        segment.timeline_start + segment.timeline_duration,
                        self.duration_tl,
                        bounds.width,
                    );

                    if (x - start_x).abs() <= edge_threshold_px {
                        state.drag_mode = Some(DragMode::TrimStart {
                            segment_id: segment.id,
                        });
                        return (canvas::event::Status::Captured, None);
                    }
                    if (x - end_x).abs() <= edge_threshold_px {
                        state.drag_mode = Some(DragMode::TrimEnd {
                            segment_id: segment.id,
                        });
                        return (canvas::event::Status::Captured, None);
                    }

                    let grab_offset_tl = tick - segment.timeline_start;
                    state.drag_mode = Some(DragMode::Move {
                        segment_id: segment.id,
                        grab_offset_tl,
                    });
                    return (canvas::event::Status::Captured, None);
                }

                state.drag_mode = Some(DragMode::Scrub);
                (canvas::event::Status::Captured, Some((self.on_scrub)(tick)))
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let Some(drag_mode) = state.drag_mode.take() else {
                    return (canvas::event::Status::Ignored, None);
                };
                let x = cursor_x.unwrap_or(0.0);
                let tick = tick_from_x(x, bounds.width, self.duration_tl);
                let drag_distance = state
                    .drag_start_x
                    .take()
                    .map(|start_x| (x - start_x).abs())
                    .unwrap_or(0.0);
                let message = match drag_mode {
                    DragMode::Scrub => None,
                    DragMode::Move {
                        segment_id,
                        grab_offset_tl,
                    } => {
                        if drag_distance < DRAG_START_THRESHOLD_PX {
                            Some((self.on_scrub)(tick))
                        } else {
                            Some((self.on_move)(segment_id, tick - grab_offset_tl))
                        }
                    }
                    DragMode::TrimStart { segment_id } => {
                        if drag_distance < DRAG_START_THRESHOLD_PX {
                            Some((self.on_scrub)(tick))
                        } else {
                            Some((self.on_trim_start)(segment_id, tick))
                        }
                    }
                    DragMode::TrimEnd { segment_id } => {
                        if drag_distance < DRAG_START_THRESHOLD_PX {
                            Some((self.on_scrub)(tick))
                        } else {
                            let end_tl = (tick + 1).clamp(1, self.duration_tl);
                            Some((self.on_trim_end)(segment_id, end_tl))
                        }
                    }
                };
                (canvas::event::Status::Captured, message)
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. })
                if matches!(state.drag_mode, Some(DragMode::Scrub)) =>
            {
                if !cursor.is_over(bounds) {
                    state.drag_mode = None;
                    state.drag_start_x = None;
                    return (canvas::event::Status::Ignored, None);
                }

                let Some(x) = cursor_x else {
                    return (canvas::event::Status::Ignored, None);
                };
                let tick = tick_from_x(x, bounds.width, self.duration_tl);
                (canvas::event::Status::Captured, Some((self.on_scrub)(tick)))
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                if !cursor.is_over(bounds) {
                    return (canvas::event::Status::Ignored, None);
                }
                let Some(x) = cursor_x else {
                    return (canvas::event::Status::Ignored, None);
                };
                let tick = tick_from_x(x, bounds.width, self.duration_tl);
                (canvas::event::Status::Captured, Some((self.on_split)(tick)))
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle)) => {
                if !cursor.is_over(bounds) {
                    return (canvas::event::Status::Ignored, None);
                }
                let Some(x) = cursor_x else {
                    return (canvas::event::Status::Ignored, None);
                };
                let tick = tick_from_x(x, bounds.width, self.duration_tl);
                (canvas::event::Status::Captured, Some((self.on_cut)(tick)))
            }
            _ => (canvas::event::Status::Ignored, None),
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let segments = self.cache.draw(renderer, bounds.size(), |frame| {
            let background = Path::rectangle(Point::ORIGIN, frame.size());
            frame.fill(&background, Color::from_rgb8(22, 24, 29));

            if self.duration_tl <= 0 {
                return;
            }

            for segment in self.segments {
                let x =
                    (segment.timeline_start.max(0) as f32 / self.duration_tl as f32) * bounds.width;
                let width = (segment.timeline_duration.max(1) as f32 / self.duration_tl as f32)
                    * bounds.width;
                let rect = Path::rectangle(
                    Point::new(x, SEGMENT_VERTICAL_PADDING_PX),
                    Size::new(
                        width.max(1.0),
                        (bounds.height - SEGMENT_VERTICAL_PADDING_PX * 2.0).max(1.0),
                    ),
                );
                frame.fill(&rect, Color::from_rgb8(55, 110, 188));
            }

            for split_tl in split_boundary_ticks(self.segments, self.duration_tl) {
                let split_x = playhead_x_from_tick(split_tl, self.duration_tl, bounds.width);
                let split_line = Path::line(
                    Point::new(split_x, SEGMENT_VERTICAL_PADDING_PX),
                    Point::new(
                        split_x,
                        (bounds.height - SEGMENT_VERTICAL_PADDING_PX)
                            .max(SEGMENT_VERTICAL_PADDING_PX),
                    ),
                );
                frame.stroke(
                    &split_line,
                    Stroke::default()
                        .with_width(1.0)
                        .with_color(Color::from_rgb8(196, 206, 220)),
                );
            }
        });

        let mut playhead_frame = canvas::Frame::new(renderer, bounds.size());
        if self.duration_tl > 0 {
            if let Some(split_tl) = self.split_feedback_tl {
                let split_x = playhead_x_from_tick(split_tl, self.duration_tl, bounds.width);
                let split_line = Path::line(
                    Point::new(split_x, 3.0),
                    Point::new(split_x, (bounds.height - 3.0).max(3.0)),
                );
                playhead_frame.stroke(
                    &split_line,
                    Stroke::default()
                        .with_width(2.0)
                        .with_color(Color::from_rgb8(122, 214, 110)),
                );
            }

            let x = playhead_x_from_tick(self.playhead_tl, self.duration_tl, bounds.width);
            let line = Path::line(Point::new(x, 0.0), Point::new(x, bounds.height));
            playhead_frame.stroke(
                &line,
                Stroke::default()
                    .with_width(2.0)
                    .with_color(Color::from_rgb8(255, 94, 77)),
            );
        }

        vec![segments, playhead_frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if self.duration_tl > 0 && cursor.is_over(bounds) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::None
        }
    }
}

/// Renders an interactive timeline canvas.
pub fn view<'a, Message>(
    snapshot: Option<&'a ProjectSnapshot>,
    playhead_tl: i64,
    split_feedback_tl: Option<i64>,
    cache: &'a canvas::Cache,
    on_scrub: fn(i64) -> Message,
    on_split: fn(i64) -> Message,
    on_cut: fn(i64) -> Message,
    on_move: fn(u64, i64) -> Message,
    on_trim_start: fn(u64, i64) -> Message,
    on_trim_end: fn(u64, i64) -> Message,
) -> Element<'a, Message>
where
    Message: 'a,
{
    let (segments, duration_tl): (&'a [SegmentSummary], i64) = match snapshot {
        Some(project) => (project.segments.as_slice(), project.duration_tl),
        None => (&[], 0),
    };

    container(
        canvas::Canvas::new(TimelineProgram {
            duration_tl,
            playhead_tl,
            split_feedback_tl,
            segments,
            cache,
            on_scrub,
            on_split,
            on_cut,
            on_move,
            on_trim_start,
            on_trim_end,
        })
        .width(Length::Fill)
        .height(Length::Fixed(56.0)),
    )
    .width(Length::Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use engine::api::SegmentSummary;
    use iced::widget::canvas;
    use iced::widget::canvas::Program;
    use iced::{Point, Rectangle, mouse};

    use super::{DragMode, TimelineProgram, TimelineState};
    use super::{playhead_x_from_tick, split_boundary_ticks, tick_from_x};

    fn sample_segment(id: u64, timeline_start: i64, timeline_duration: i64) -> SegmentSummary {
        SegmentSummary {
            id,
            asset_id: 1,
            timeline_start,
            timeline_duration,
            src_in_video: None,
            src_out_video: None,
            src_in_audio: None,
            src_out_audio: None,
        }
    }

    #[test]
    fn maps_left_edge_to_zero() {
        assert_eq!(tick_from_x(0.0, 200.0, 1_000), 0);
    }

    #[test]
    fn clamps_negative_position_to_zero() {
        assert_eq!(tick_from_x(-10.0, 200.0, 1_000), 0);
    }

    #[test]
    fn maps_right_edge_to_last_tick() {
        assert_eq!(tick_from_x(200.0, 200.0, 1_000), 999);
    }

    #[test]
    fn maps_middle_position_proportionally() {
        assert_eq!(tick_from_x(100.0, 200.0, 1_000), 500);
    }

    #[test]
    fn handles_empty_timeline_as_zero() {
        assert_eq!(tick_from_x(100.0, 200.0, 0), 0);
    }

    #[test]
    fn playhead_x_uses_same_duration_scale_as_tick_mapping() {
        assert_eq!(playhead_x_from_tick(1, 2, 200.0), 200.0);
    }

    #[test]
    fn non_last_tick_keeps_proportional_position() {
        assert_eq!(playhead_x_from_tick(1, 4, 200.0), 50.0);
    }

    #[test]
    fn split_boundaries_include_all_non_zero_segment_starts() {
        let segments = vec![
            sample_segment(1, 0, 100),
            sample_segment(2, 100, 100),
            sample_segment(3, 250, 150),
        ];

        assert_eq!(split_boundary_ticks(&segments, 400), vec![100, 250]);
    }

    #[test]
    fn split_boundaries_skip_out_of_range_and_duplicate_starts() {
        let segments = vec![
            sample_segment(1, -10, 10),
            sample_segment(2, 0, 100),
            sample_segment(3, 100, 100),
            sample_segment(4, 100, 100),
            sample_segment(5, 400, 100),
            sample_segment(6, 500, 100),
        ];

        assert_eq!(split_boundary_ticks(&segments, 400), vec![100]);
    }

    #[test]
    fn mouse_interaction_is_none_when_timeline_is_empty() {
        let cache = iced::widget::canvas::Cache::new();
        let program = TimelineProgram {
            duration_tl: 0,
            playhead_tl: 0,
            split_feedback_tl: None,
            segments: &[],
            cache: &cache,
            on_scrub: |_| (),
            on_split: |_| (),
            on_cut: |_| (),
            on_move: |_, _| (),
            on_trim_start: |_, _| (),
            on_trim_end: |_, _| (),
        };
        let interaction = program.mouse_interaction(
            &TimelineState::default(),
            Rectangle {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 40.0,
            },
            mouse::Cursor::Available(Point::new(20.0, 10.0)),
        );

        assert_eq!(interaction, mouse::Interaction::None);
    }

    #[test]
    fn mouse_interaction_is_pointer_when_timeline_is_interactive() {
        let cache = iced::widget::canvas::Cache::new();
        let program = TimelineProgram {
            duration_tl: 10,
            playhead_tl: 0,
            split_feedback_tl: None,
            segments: &[],
            cache: &cache,
            on_scrub: |_| (),
            on_split: |_| (),
            on_cut: |_| (),
            on_move: |_, _| (),
            on_trim_start: |_, _| (),
            on_trim_end: |_, _| (),
        };
        let interaction = program.mouse_interaction(
            &TimelineState::default(),
            Rectangle {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 40.0,
            },
            mouse::Cursor::Available(Point::new(20.0, 10.0)),
        );

        assert_eq!(interaction, mouse::Interaction::Pointer);
    }

    #[test]
    fn drag_stops_when_cursor_leaves_timeline_bounds() {
        let cache = iced::widget::canvas::Cache::new();
        let program = TimelineProgram {
            duration_tl: 100,
            playhead_tl: 0,
            split_feedback_tl: None,
            segments: &[],
            cache: &cache,
            on_scrub: |tick| tick,
            on_split: |_| -1,
            on_cut: |_| -2,
            on_move: |_, _| -3,
            on_trim_start: |_, _| -4,
            on_trim_end: |_, _| -5,
        };
        let bounds = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let mut state = TimelineState::default();

        let (_, pressed) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            bounds,
            mouse::Cursor::Available(Point::new(10.0, 10.0)),
        );
        assert_eq!(pressed, Some(10));
        assert!(state.drag_mode.is_some());

        let (status, moved) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::CursorMoved {
                position: Point::new(120.0, 10.0),
            }),
            bounds,
            mouse::Cursor::Available(Point::new(120.0, 10.0)),
        );

        assert_eq!(status, canvas::event::Status::Ignored);
        assert_eq!(moved, None);
        assert!(state.drag_mode.is_none());
    }

    #[test]
    fn left_click_outside_timeline_does_not_seek() {
        let cache = iced::widget::canvas::Cache::new();
        let program = TimelineProgram {
            duration_tl: 100,
            playhead_tl: 0,
            split_feedback_tl: None,
            segments: &[],
            cache: &cache,
            on_scrub: |tick| tick,
            on_split: |_| -1,
            on_cut: |_| -2,
            on_move: |_, _| -3,
            on_trim_start: |_, _| -4,
            on_trim_end: |_, _| -5,
        };
        let bounds = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let mut state = TimelineState::default();

        let (status, message) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            bounds,
            mouse::Cursor::Available(Point::new(120.0, 10.0)),
        );

        assert_eq!(status, canvas::event::Status::Ignored);
        assert_eq!(message, None);
        assert!(state.drag_mode.is_none());
    }

    #[test]
    fn right_click_outside_timeline_does_not_split() {
        let cache = iced::widget::canvas::Cache::new();
        let program = TimelineProgram {
            duration_tl: 100,
            playhead_tl: 0,
            split_feedback_tl: None,
            segments: &[],
            cache: &cache,
            on_scrub: |_| -1,
            on_split: |tick| tick,
            on_cut: |_| -2,
            on_move: |_, _| -3,
            on_trim_start: |_, _| -4,
            on_trim_end: |_, _| -5,
        };
        let bounds = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let mut state = TimelineState::default();

        let (status, message) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)),
            bounds,
            mouse::Cursor::Available(Point::new(-10.0, 10.0)),
        );

        assert_eq!(status, canvas::event::Status::Ignored);
        assert_eq!(message, None);
    }

    #[test]
    fn middle_click_on_timeline_dispatches_cut() {
        let cache = iced::widget::canvas::Cache::new();
        let program = TimelineProgram {
            duration_tl: 100,
            playhead_tl: 0,
            split_feedback_tl: None,
            segments: &[],
            cache: &cache,
            on_scrub: |_| -1,
            on_split: |_| -2,
            on_cut: |tick| tick,
            on_move: |_, _| -3,
            on_trim_start: |_, _| -4,
            on_trim_end: |_, _| -5,
        };
        let bounds = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let mut state = TimelineState::default();

        let (status, message) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle)),
            bounds,
            mouse::Cursor::Available(Point::new(25.0, 10.0)),
        );

        assert_eq!(status, canvas::event::Status::Captured);
        assert_eq!(message, Some(25));
    }

    #[test]
    fn middle_click_outside_timeline_does_not_cut() {
        let cache = iced::widget::canvas::Cache::new();
        let program = TimelineProgram {
            duration_tl: 100,
            playhead_tl: 0,
            split_feedback_tl: None,
            segments: &[],
            cache: &cache,
            on_scrub: |_| -1,
            on_split: |_| -2,
            on_cut: |tick| tick,
            on_move: |_, _| -3,
            on_trim_start: |_, _| -4,
            on_trim_end: |_, _| -5,
        };
        let bounds = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let mut state = TimelineState::default();

        let (status, message) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle)),
            bounds,
            mouse::Cursor::Available(Point::new(-10.0, 10.0)),
        );

        assert_eq!(status, canvas::event::Status::Ignored);
        assert_eq!(message, None);
    }

    #[test]
    fn drag_segment_body_dispatches_move_on_release() {
        let cache = iced::widget::canvas::Cache::new();
        let segments = vec![sample_segment(7, 20, 40)];
        let program = TimelineProgram {
            duration_tl: 100,
            playhead_tl: 0,
            split_feedback_tl: None,
            segments: &segments,
            cache: &cache,
            on_scrub: |_| -1,
            on_split: |_| -2,
            on_cut: |_| -3,
            on_move: |segment_id, start_tl| segment_id as i64 * 1_000 + start_tl,
            on_trim_start: |_, _| -4,
            on_trim_end: |_, _| -5,
        };
        let bounds = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let mut state = TimelineState::default();

        let (_, pressed) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            bounds,
            mouse::Cursor::Available(Point::new(30.0, 10.0)),
        );
        assert_eq!(pressed, None);

        let (status, released) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            bounds,
            mouse::Cursor::Available(Point::new(50.0, 10.0)),
        );
        assert_eq!(status, canvas::event::Status::Captured);
        assert_eq!(released, Some(7_040));
    }

    #[test]
    fn click_segment_body_dispatches_scrub_instead_of_move() {
        let cache = iced::widget::canvas::Cache::new();
        let segments = vec![sample_segment(7, 20, 40)];
        let program = TimelineProgram {
            duration_tl: 100,
            playhead_tl: 0,
            split_feedback_tl: None,
            segments: &segments,
            cache: &cache,
            on_scrub: |tick| tick,
            on_split: |_| -2,
            on_cut: |_| -3,
            on_move: |_, _| -4,
            on_trim_start: |_, _| -5,
            on_trim_end: |_, _| -6,
        };
        let bounds = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let mut state = TimelineState::default();

        let (_, pressed) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            bounds,
            mouse::Cursor::Available(Point::new(30.0, 10.0)),
        );
        assert_eq!(pressed, None);

        let (status, released) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            bounds,
            mouse::Cursor::Available(Point::new(30.0, 10.0)),
        );
        assert_eq!(status, canvas::event::Status::Captured);
        assert_eq!(released, Some(30));
    }

    #[test]
    fn click_segment_body_in_black_area_dispatches_scrub() {
        let cache = iced::widget::canvas::Cache::new();
        let segments = vec![sample_segment(7, 20, 40)];
        let program = TimelineProgram {
            duration_tl: 100,
            playhead_tl: 0,
            split_feedback_tl: None,
            segments: &segments,
            cache: &cache,
            on_scrub: |tick| tick,
            on_split: |_| -2,
            on_cut: |_| -3,
            on_move: |_, _| -4,
            on_trim_start: |_, _| -5,
            on_trim_end: |_, _| -6,
        };
        let bounds = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let mut state = TimelineState::default();

        let (status, pressed) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            bounds,
            mouse::Cursor::Available(Point::new(30.0, 3.0)),
        );

        assert_eq!(status, canvas::event::Status::Captured);
        assert_eq!(pressed, Some(30));
        assert!(matches!(state.drag_mode, Some(DragMode::Scrub)));
    }

    #[test]
    fn click_segment_start_edge_in_black_area_dispatches_scrub_instead_of_trim() {
        let cache = iced::widget::canvas::Cache::new();
        let segments = vec![sample_segment(7, 20, 40)];
        let program = TimelineProgram {
            duration_tl: 100,
            playhead_tl: 0,
            split_feedback_tl: None,
            segments: &segments,
            cache: &cache,
            on_scrub: |tick| tick,
            on_split: |_| -2,
            on_cut: |_| -3,
            on_move: |_, _| -4,
            on_trim_start: |_, _| -5,
            on_trim_end: |_, _| -6,
        };
        let bounds = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let mut state = TimelineState::default();

        let (status, pressed) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            bounds,
            mouse::Cursor::Available(Point::new(20.0, 3.0)),
        );

        assert_eq!(status, canvas::event::Status::Captured);
        assert_eq!(pressed, Some(20));
        assert!(matches!(state.drag_mode, Some(DragMode::Scrub)));
    }

    #[test]
    fn click_segment_body_near_top_margin_dispatches_scrub() {
        let cache = iced::widget::canvas::Cache::new();
        let segments = vec![sample_segment(7, 20, 40)];
        let program = TimelineProgram {
            duration_tl: 100,
            playhead_tl: 0,
            split_feedback_tl: None,
            segments: &segments,
            cache: &cache,
            on_scrub: |tick| tick,
            on_split: |_| -2,
            on_cut: |_| -3,
            on_move: |_, _| -4,
            on_trim_start: |_, _| -5,
            on_trim_end: |_, _| -6,
        };
        let bounds = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let mut state = TimelineState::default();

        let (status, pressed) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            bounds,
            mouse::Cursor::Available(Point::new(30.0, 9.0)),
        );

        assert_eq!(status, canvas::event::Status::Captured);
        assert_eq!(pressed, Some(30));
        assert!(matches!(state.drag_mode, Some(DragMode::Scrub)));
    }

    #[test]
    fn drag_segment_start_edge_dispatches_trim_start_on_release() {
        let cache = iced::widget::canvas::Cache::new();
        let segments = vec![sample_segment(7, 20, 40)];
        let program = TimelineProgram {
            duration_tl: 100,
            playhead_tl: 0,
            split_feedback_tl: None,
            segments: &segments,
            cache: &cache,
            on_scrub: |_| -1,
            on_split: |_| -2,
            on_cut: |_| -3,
            on_move: |_, _| -4,
            on_trim_start: |segment_id, start_tl| segment_id as i64 * 1_000 + start_tl,
            on_trim_end: |_, _| -5,
        };
        let bounds = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let mut state = TimelineState::default();

        let (_, pressed) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            bounds,
            mouse::Cursor::Available(Point::new(20.0, 10.0)),
        );
        assert_eq!(pressed, None);

        let (status, released) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            bounds,
            mouse::Cursor::Available(Point::new(25.0, 10.0)),
        );
        assert_eq!(status, canvas::event::Status::Captured);
        assert_eq!(released, Some(7_025));
    }

    #[test]
    fn click_segment_start_edge_dispatches_scrub_instead_of_trim() {
        let cache = iced::widget::canvas::Cache::new();
        let segments = vec![sample_segment(7, 20, 40)];
        let program = TimelineProgram {
            duration_tl: 100,
            playhead_tl: 0,
            split_feedback_tl: None,
            segments: &segments,
            cache: &cache,
            on_scrub: |tick| tick,
            on_split: |_| -2,
            on_cut: |_| -3,
            on_move: |_, _| -4,
            on_trim_start: |_, _| -5,
            on_trim_end: |_, _| -6,
        };
        let bounds = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let mut state = TimelineState::default();

        let (_, pressed) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            bounds,
            mouse::Cursor::Available(Point::new(20.0, 10.0)),
        );
        assert_eq!(pressed, None);

        let (status, released) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            bounds,
            mouse::Cursor::Available(Point::new(20.0, 10.0)),
        );
        assert_eq!(status, canvas::event::Status::Captured);
        assert_eq!(released, Some(20));
    }

    #[test]
    fn drag_segment_end_edge_dispatches_trim_end_on_release() {
        let cache = iced::widget::canvas::Cache::new();
        let segments = vec![sample_segment(7, 20, 40)];
        let program = TimelineProgram {
            duration_tl: 100,
            playhead_tl: 0,
            split_feedback_tl: None,
            segments: &segments,
            cache: &cache,
            on_scrub: |_| -1,
            on_split: |_| -2,
            on_cut: |_| -3,
            on_move: |_, _| -4,
            on_trim_start: |_, _| -5,
            on_trim_end: |segment_id, end_tl| segment_id as i64 * 1_000 + end_tl,
        };
        let bounds = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let mut state = TimelineState::default();

        let (_, pressed) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            bounds,
            mouse::Cursor::Available(Point::new(60.0, 10.0)),
        );
        assert_eq!(pressed, None);

        let (status, released) = program.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            bounds,
            mouse::Cursor::Available(Point::new(70.0, 10.0)),
        );
        assert_eq!(status, canvas::event::Status::Captured);
        assert_eq!(released, Some(7_070));
    }
}
