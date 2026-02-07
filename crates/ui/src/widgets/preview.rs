use engine::{PreviewFrame, PreviewPixelFormat};
use iced::widget::{container, image, text};
use iced::{ContentFit, Element, Length};

/// UI-ready preview image converted from an engine frame.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewImage {
    pub handle: image::Handle,
    pub width: u32,
    pub height: u32,
}

impl PreviewImage {
    /// Converts an RGBA frame into an iced image handle.
    pub fn from_frame(frame: &PreviewFrame) -> Option<Self> {
        if frame.format != PreviewPixelFormat::Rgba8 {
            return None;
        }

        let expected_bytes = frame.width.checked_mul(frame.height)?.checked_mul(4)? as usize;
        if frame.bytes.len() != expected_bytes {
            return None;
        }

        Some(Self {
            handle: image::Handle::from_rgba(frame.width, frame.height, frame.bytes.to_vec()),
            width: frame.width,
            height: frame.height,
        })
    }
}

/// Renders the preview area.
pub fn view<'a, Message>(latest: Option<&PreviewImage>) -> Element<'a, Message>
where
    Message: 'a,
{
    match latest {
        Some(image_data) => image(image_data.handle.clone())
            .content_fit(ContentFit::Contain)
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
        None => container(text("No preview frame"))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use engine::{PreviewFrame, PreviewPixelFormat};
    use iced::widget::image;

    use super::PreviewImage;

    #[test]
    fn converts_rgba_frame_into_image_handle() {
        let frame = PreviewFrame {
            width: 2,
            height: 1,
            format: PreviewPixelFormat::Rgba8,
            bytes: Arc::from(vec![0_u8, 1, 2, 3, 4, 5, 6, 7]),
        };

        let Some(image) = PreviewImage::from_frame(&frame) else {
            panic!("expected preview image");
        };

        let image::Handle::Rgba {
            width,
            height,
            pixels,
            ..
        } = image.handle
        else {
            panic!("expected rgba handle");
        };
        assert_eq!(width, 2);
        assert_eq!(height, 1);
        assert_eq!(pixels.len(), 8);
    }

    #[test]
    fn rejects_frame_with_invalid_rgba_byte_length() {
        let frame = PreviewFrame {
            width: 2,
            height: 2,
            format: PreviewPixelFormat::Rgba8,
            bytes: Arc::from(vec![0_u8; 3]),
        };

        assert!(PreviewImage::from_frame(&frame).is_none());
    }

    #[test]
    fn rejects_non_rgba_frame_for_mvp_widget_path() {
        let frame = PreviewFrame {
            width: 2,
            height: 2,
            format: PreviewPixelFormat::Nv12,
            bytes: Arc::from(vec![0_u8; 8]),
        };

        assert!(PreviewImage::from_frame(&frame).is_none());
    }
}
