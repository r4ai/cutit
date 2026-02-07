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
    dragging: bool,
}

#[derive(Debug)]
struct TimelineProgram<'a, Message> {
    duration_tl: i64,
    playhead_tl: i64,
    split_feedback_tl: Option<i64>,
    segments: &'a [SegmentSummary],
    cache: &'a canvas::Cache,
    on_scrub: fn(i64) -> Message,
    on_split: fn(i64) -> Message,
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
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(x) = cursor_x else {
                    return (canvas::event::Status::Ignored, None);
                };
                state.dragging = true;
                let tick = tick_from_x(x, bounds.width, self.duration_tl);
                (canvas::event::Status::Captured, Some((self.on_scrub)(tick)))
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let was_dragging = state.dragging;
                state.dragging = false;
                if was_dragging {
                    (canvas::event::Status::Captured, None)
                } else {
                    (canvas::event::Status::Ignored, None)
                }
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) if state.dragging => {
                let Some(x) = cursor_x else {
                    return (canvas::event::Status::Ignored, None);
                };
                let tick = tick_from_x(x, bounds.width, self.duration_tl);
                (canvas::event::Status::Captured, Some((self.on_scrub)(tick)))
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                let Some(x) = cursor_x else {
                    return (canvas::event::Status::Ignored, None);
                };
                let tick = tick_from_x(x, bounds.width, self.duration_tl);
                (canvas::event::Status::Captured, Some((self.on_split)(tick)))
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
                    Point::new(x, 8.0),
                    Size::new(width.max(1.0), (bounds.height - 16.0).max(1.0)),
                );
                frame.fill(&rect, Color::from_rgb8(55, 110, 188));
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
        })
        .width(Length::Fill)
        .height(Length::Fixed(56.0)),
    )
    .width(Length::Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use iced::widget::canvas::Program;
    use iced::{Point, Rectangle, mouse};

    use super::{TimelineProgram, TimelineState};
    use super::{playhead_x_from_tick, tick_from_x};

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
}
