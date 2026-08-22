mod kde;
mod niri;

use clap::ValueEnum;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendPreference {
    Niri,
    Kde,
}

pub struct DesktopBackend {
    kde: Option<kde::KdeBackend>,
    niri: Option<niri::NiriBackend>,
    preference: Option<BackendPreference>,
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
    pub fn detect(preference: Option<BackendPreference>) -> Self {
        Self {
            kde: (preference != Some(BackendPreference::Niri))
                .then(kde::KdeBackend::detect)
                .flatten(),
            niri: (preference != Some(BackendPreference::Kde))
                .then(niri::NiriBackend::detect)
                .flatten(),
            preference,
        }
    }

    pub fn capture_session(&self) -> Result<CaptureSession> {
        let wayland = WaylandCapture::connect()?;
        match self.preference {
            Some(BackendPreference::Kde) => {
                let capture = self.kde.clone().ok_or_else(|| {
                    Error::BackendUnavailable(
                        "KDE was selected with --be, but org.kde.KWin.ScreenShot2 was not found"
                            .into(),
                    )
                })?;
                Ok(CaptureSession::Kde {
                    outputs: wayland.outputs(),
                    capture,
                })
            }
            Some(BackendPreference::Niri) => {
                if self.niri.is_none() {
                    return Err(Error::BackendUnavailable(
                        "niri was selected with --be, but NIRI_SOCKET is not set".into(),
                    ));
                }
                if !wayland.supports_screencopy() {
                    return Err(Error::BackendUnavailable(
                        "niri was selected with --be, but wlr-screencopy-unstable-v1 is unavailable"
                            .into(),
                    ));
                }
                Ok(CaptureSession::Wlr(Box::new(wayland)))
            }
            None if wayland.supports_screencopy() => Ok(CaptureSession::Wlr(Box::new(wayland))),
            None => {
                if let Some(capture) = self.kde.clone() {
                    Ok(CaptureSession::Kde {
                        outputs: wayland.outputs(),
                        capture,
                    })
                } else {
                    Err(Error::MissingProtocol("wlr-screencopy-unstable-v1"))
                }
            }
        }
    }

    pub fn focused_output_name(&self) -> Option<String> {
        if self.preference == Some(BackendPreference::Kde) {
            return None;
        }
        self.niri
            .as_ref()
            .and_then(|backend| backend.focused_output_name().ok().flatten())
    }

    pub fn capture_focused_window(&self, show_pointer: bool) -> Result<WindowCapture> {
        match self.preference {
            Some(BackendPreference::Niri) => {
                let backend = self.niri.as_ref().ok_or_else(|| {
                    Error::BackendUnavailable(
                        "niri was selected with --be, but NIRI_SOCKET is not set".into(),
                    )
                })?;
                niri_window_capture(backend, show_pointer)
            }
            Some(BackendPreference::Kde) => {
                let backend = self.kde.as_ref().ok_or_else(|| {
                    Error::BackendUnavailable(
                        "KDE was selected with --be, but org.kde.KWin.ScreenShot2 was not found"
                            .into(),
                    )
                })?;
                kde_window_capture(backend, show_pointer)
            }
            None => {
                if let Some(backend) = &self.niri {
                    niri_window_capture(backend, show_pointer)
                } else if let Some(backend) = &self.kde {
                    kde_window_capture(backend, show_pointer)
                } else {
                    Err(Error::WindowCaptureUnsupported)
                }
            }
        }
    }
}

fn niri_window_capture(backend: &niri::NiriBackend, show_pointer: bool) -> Result<WindowCapture> {
    Ok(WindowCapture {
        image: backend.capture_focused_window(show_pointer)?,
        // ScreenshotWindow always sets niri's clipboard and offers no switch to disable it.
        clipboard_already_set: true,
    })
}

fn kde_window_capture(backend: &kde::KdeBackend, show_pointer: bool) -> Result<WindowCapture> {
    Ok(WindowCapture {
        image: backend.capture_active_window(show_pointer)?,
        clipboard_already_set: false,
    })
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
