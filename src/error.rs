use std::{io, path::PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("niri IPC: {0}")]
    Niri(String),
    #[error("KWin screenshot: {0}")]
    Kde(String),
    #[error("requested compositor backend is unavailable: {0}")]
    BackendUnavailable(String),
    #[error("focused-window capture is unavailable: no supported compositor backend was detected")]
    WindowCaptureUnsupported,
    #[error("Wayland: {0}")]
    Wayland(String),
    #[error("required Wayland protocol is unavailable: {0}")]
    MissingProtocol(&'static str),
    #[error("output {0:?} was not found")]
    OutputNotFound(String),
    #[error("output name is required (use `dmsshot output NAME` or --output NAME)")]
    MissingOutputName,
    #[error("no outputs are available")]
    NoOutputs,
    #[error("capture was rejected by the compositor")]
    CaptureFailed,
    #[error("invalid capture buffer: {0}")]
    InvalidBuffer(String),
    #[error("scroll capture does not support rotated outputs")]
    RotatedScroll,
    #[error("image is too large")]
    ImageTooLarge,
    #[error("unable to create output directory {path}: {source}")]
    CreateOutputDir { path: PathBuf, source: io::Error },
    #[error("unable to write {path}: {source}")]
    WriteFile { path: PathBuf, source: io::Error },
    #[error("clipboard: {0}")]
    Clipboard(String),
    #[error("OCR engine `tesseract` was not found; install Tesseract and its language data")]
    TesseractUnavailable,
    #[error(
        "PaddleOCR models were not found in {searched}; expected {missing}. Use `ocr --model-dir DIR` or set DMSSHOT_OCR_MODEL_DIR"
    )]
    PaddleModelsMissing { searched: String, missing: String },
    #[error("OCR failed: {0}")]
    Ocr(String),
    #[error("OCR did not recognize any text")]
    OcrNoText,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Image(#[from] image::ImageError),
}

pub type Result<T> = std::result::Result<T, Error>;
