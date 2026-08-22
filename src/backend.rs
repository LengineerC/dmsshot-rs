mod kde;
mod niri;

use crate::{
    error::{Error, Result},
    imagebuf::ImageBuffer,
    wayland::WaylandCapture,
};

#[derive(Debug, Clone, PartialEq)]
pub struct OutputInfo {
    pub id: u32,
    pub name: String,
    pub physical_width: u32,
    pub physical_height: u32,
    pub x: i32,
    pub y: i32,
    pub logical_width: u32,
    pub logical_height: u32,
    pub scale: f64,
    pub transform: i32,
}

pub struct WindowCapture {
    pub image: ImageBuffer,
    pub clipboard_already_set: bool,
}

pub struct DesktopBackend {
    kde: Option<kde::KdeBackend>,
    niri: Option<niri::NiriBackend>,
}

pub trait CaptureSource {
    fn capture_output(&mut self, info: &OutputInfo, overlay_cursor: bool) -> Result<ImageBuffer>;

    fn capture_region(
        &mut self,
        info: &OutputInfo,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        overlay_cursor: bool,
    ) -> Result<ImageBuffer>;
}

pub enum CaptureSession {
    Wlr(Box<WaylandCapture>),
    Kde {
        capture: kde::KdeBackend,
        outputs: Vec<OutputInfo>,
    },
}

impl DesktopBackend {
    pub fn detect() -> Self {
        Self {
            kde: kde::KdeBackend::detect(),
            niri: niri::NiriBackend::detect(),
        }
    }

    pub fn capture_session(&self) -> Result<CaptureSession> {
        let wayland = WaylandCapture::connect()?;
        if wayland.supports_screencopy() {
            return Ok(CaptureSession::Wlr(Box::new(wayland)));
        }
        if let Some(capture) = self.kde.clone() {
            return Ok(CaptureSession::Kde {
                outputs: wayland.outputs(),
                capture,
            });
        }
        Err(Error::MissingProtocol("wlr-screencopy-unstable-v1"))
    }

    pub fn focused_output_name(&self) -> Option<String> {
        self.niri
            .as_ref()
            .and_then(|backend| backend.focused_output_name().ok().flatten())
    }

    pub fn capture_focused_window(&self, show_pointer: bool) -> Result<WindowCapture> {
        if let Some(backend) = &self.niri {
            return Ok(WindowCapture {
                image: backend.capture_focused_window(show_pointer)?,
                // ScreenshotWindow always sets niri's clipboard and offers no switch to disable it.
                clipboard_already_set: true,
            });
        }
        if let Some(backend) = &self.kde {
            return Ok(WindowCapture {
                image: backend.capture_active_window(show_pointer)?,
                clipboard_already_set: false,
            });
        }
        Err(Error::WindowCaptureUnsupported)
    }
}

impl CaptureSession {
    pub fn outputs(&self) -> Vec<OutputInfo> {
        match self {
            Self::Wlr(capture) => capture.outputs(),
            Self::Kde { outputs, .. } => outputs.clone(),
        }
    }

    pub fn capture_active_output(&mut self, overlay_cursor: bool) -> Result<Option<ImageBuffer>> {
        match self {
            Self::Wlr(_) => Ok(None),
            Self::Kde { capture, outputs } => capture
                .capture_active_screen(outputs, overlay_cursor)
                .map(Some),
        }
    }

    pub fn capture_workspace(&mut self, overlay_cursor: bool) -> Result<Option<ImageBuffer>> {
        match self {
            Self::Wlr(_) => Ok(None),
            Self::Kde { capture, outputs } => {
                capture.capture_workspace(outputs, overlay_cursor).map(Some)
            }
        }
    }
}

impl CaptureSource for CaptureSession {
    fn capture_output(&mut self, info: &OutputInfo, overlay_cursor: bool) -> Result<ImageBuffer> {
        match self {
            Self::Wlr(capture) => capture.capture_output(info, overlay_cursor),
            Self::Kde { capture, .. } => capture.capture_screen(info, overlay_cursor),
        }
    }

    fn capture_region(
        &mut self,
        info: &OutputInfo,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        overlay_cursor: bool,
    ) -> Result<ImageBuffer> {
        match self {
            Self::Wlr(capture) => capture.capture_region(info, x, y, width, height, overlay_cursor),
            Self::Kde { capture, .. } => {
                capture.capture_area(info.x + x, info.y + y, width, height, info, overlay_cursor)
            }
        }
    }
}
