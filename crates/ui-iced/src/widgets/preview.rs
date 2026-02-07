use std::sync::Arc;

use engine::{PreviewFrame, PreviewPixelFormat};

/// Renderable preview payload kept on the UI side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewImage {
    pub width: u32,
    pub height: u32,
    pub format: PreviewPixelFormat,
    pub bytes: Arc<[u8]>,
}

/// State of the preview widget.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreviewState {
    latest: Option<PreviewImage>,
}

impl PreviewState {
    /// Stores a new frame as the latest render target.
    pub fn push_frame(&mut self, frame: PreviewFrame) {
        self.latest = Some(PreviewImage {
            width: frame.width,
            height: frame.height,
            format: frame.format,
            bytes: frame.bytes,
        });
    }

    /// Returns the currently rendered frame, if any.
    pub fn latest(&self) -> Option<&PreviewImage> {
        self.latest.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use engine::{PreviewFrame, PreviewPixelFormat};

    use super::PreviewState;

    #[test]
    fn latest_frame_replaces_previous_frame() {
        let mut state = PreviewState::default();

        state.push_frame(PreviewFrame {
            width: 640,
            height: 360,
            format: PreviewPixelFormat::Rgba8,
            bytes: Arc::from(vec![0_u8; 640 * 360 * 4]),
        });
        state.push_frame(PreviewFrame {
            width: 1280,
            height: 720,
            format: PreviewPixelFormat::Rgba8,
            bytes: Arc::from(vec![255_u8; 1280 * 720 * 4]),
        });

        let latest = state.latest().expect("latest frame should be present");
        assert_eq!(latest.width, 1280);
        assert_eq!(latest.height, 720);
        assert_eq!(latest.bytes.len(), 1280 * 720 * 4);
    }
}
