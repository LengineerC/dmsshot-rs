use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::backend::BackendPreference;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CursorMode {
    Off,
    On,
}

impl CursorMode {
    pub fn enabled(self) -> bool {
        self == Self::On
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Png,
    #[value(alias = "jpeg")]
    Jpg,
    Ppm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorFormat {
    Hex,
    Rgb,
    Hsl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OcrEngine {
    Paddle,
    Tesseract,
}

impl OcrEngine {
    pub fn name(self) -> &'static str {
        match self {
            Self::Paddle => "paddle",
            Self::Tesseract => "tesseract",
        }
    }
}

impl OutputFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpg => "jpg",
            Self::Ppm => "ppm",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpg => "image/jpeg",
            Self::Ppm => "image/x-portable-pixmap",
        }
    }
}

#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum Command {
    /// Select a region interactively.
    Region,
    /// Capture the focused output.
    Full,
    /// Capture all outputs into one image.
    All,
    /// Capture one output by name.
    Output {
        /// Output name. This positional argument overrides --output.
        name: Option<String>,
    },
    /// Capture the focused window using an available compositor backend.
    Window,
    /// Select and continuously stitch a scrolling region.
    Scroll {
        /// Idle capture interval in milliseconds.
        #[arg(long, default_value_t = 45, value_parser = clap::value_parser!(u64).range(30..=1000))]
        interval: u64,
    },
    /// Select a region and recognize its text.
    Ocr {
        /// OCR engine. Paddle uses the bundled Chinese/English PP-OCRv4 model set.
        #[arg(long, value_enum, default_value_t = OcrEngine::Paddle)]
        engine: OcrEngine,
        /// Directory containing the PaddleOCR detection, orientation, and recognition models.
        #[arg(long, value_name = "DIR")]
        model_dir: Option<PathBuf>,
        /// Tesseract language or language combination (only used by --engine tesseract).
        #[arg(short = 'l', long = "lang", default_value = "chi_sim+eng")]
        language: String,
        /// Force a Tesseract page segmentation mode (only used by --engine tesseract).
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=13))]
        psm: Option<u8>,
    },
    /// List available outputs.
    List,
    /// Internal notification action listener.
    #[command(hide = true)]
    NotificationAction {
        path: PathBuf,
        #[arg(long)]
        copied: bool,
    },
}

#[derive(Debug, Clone, Parser)]
#[command(name = "dmsshot", version, about = "Native Wayland screenshot utility")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Output name for output mode.
    #[arg(short = 'o', long, global = true)]
    pub output: Option<String>,

    /// Include the pointer in captured images.
    #[arg(long, value_enum, default_value_t = CursorMode::Off, global = true)]
    pub cursor: CursorMode,

    /// Show the cursor magnifier in interactive selectors and enable color copying with C.
    #[arg(long, global = true)]
    pub magnifier: bool,

    /// Color text used by the magnifier and C key.
    #[arg(long, value_enum, default_value_t = ColorFormat::Hex, requires = "magnifier", global = true)]
    pub color_format: ColorFormat,

    /// Image format.
    #[arg(short = 'f', long = "format", value_enum, default_value_t = OutputFormat::Png, global = true)]
    pub format: OutputFormat,

    /// JPEG quality.
    #[arg(short = 'q', long, default_value_t = 90, value_parser = clap::value_parser!(u8).range(1..=100), global = true)]
    pub quality: u8,

    /// Directory in which to save the image.
    #[arg(short = 'd', long = "dir", global = true)]
    pub output_dir: Option<PathBuf>,

    /// Output filename.
    #[arg(long, global = true)]
    pub filename: Option<PathBuf>,

    /// Capture a region immediately after releasing the mouse.
    #[arg(long, global = true)]
    pub no_confirm: bool,

    /// Clear the saved region before opening the selector.
    #[arg(long, global = true)]
    pub reset: bool,

    /// Write the encoded image to stdout instead of a file.
    #[arg(long, conflicts_with = "json", global = true)]
    pub stdout: bool,

    /// Print machine-readable capture metadata.
    #[arg(long, global = true)]
    pub json: bool,

    /// Do not copy the captured image to the Wayland clipboard.
    #[arg(long, global = true)]
    pub no_clipboard: bool,

    /// Do not show a desktop notification after a successful capture.
    #[arg(long, global = true)]
    pub no_notify: bool,

    /// Use colors from the active GTK theme for interactive overlays (default).
    #[arg(long, conflicts_with = "qt", global = true)]
    pub gtk: bool,

    /// Use colors from qt6ct, qt5ct, or KDE for interactive overlays.
    #[arg(long, conflicts_with = "gtk", global = true)]
    pub qt: bool,

    /// Force a compositor backend instead of detecting it automatically.
    #[arg(long = "be", value_enum, value_name = "BACKEND", global = true)]
    pub backend: Option<BackendPreference>,
}

impl Cli {
    pub fn command(&self) -> Command {
        self.command.clone().unwrap_or(Command::Region)
    }

    pub fn output_name(&self, positional: Option<&str>) -> Option<String> {
        positional
            .map(ToOwned::to_owned)
            .or_else(|| self.output.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;

    use super::{BackendPreference, Cli, ColorFormat, Command, OutputFormat};

    #[test]
    fn defaults_to_region() {
        let cli = Cli::try_parse_from(["dmsshot"]).unwrap();
        assert_eq!(cli.command(), Command::Region);
        assert_eq!(cli.format, OutputFormat::Png);
        assert_eq!(cli.color_format, ColorFormat::Hex);
        assert!(!cli.magnifier);
    }

    #[test]
    fn magnifier_can_be_enabled_globally() {
        let cli = Cli::try_parse_from(["dmsshot", "--magnifier", "region"]).unwrap();
        assert!(cli.magnifier);
        let cli = Cli::try_parse_from(["dmsshot", "ocr", "--magnifier"]).unwrap();
        assert!(cli.magnifier);
    }

    #[test]
    fn color_format_is_global_and_requires_the_magnifier() {
        let cli = Cli::try_parse_from(["dmsshot", "--color-format", "rgb", "--magnifier"]).unwrap();
        assert_eq!(cli.color_format, ColorFormat::Rgb);

        let cli =
            Cli::try_parse_from(["dmsshot", "region", "--magnifier", "--color-format", "hsl"])
                .unwrap();
        assert_eq!(cli.color_format, ColorFormat::Hsl);

        assert!(Cli::try_parse_from(["dmsshot", "--color-format", "rgb"]).is_err());
        assert!(Cli::try_parse_from(["dmsshot", "--color-format", "hex", "region"]).is_err());
        assert!(Cli::try_parse_from(["dmsshot", "--magnifier", "--color-format", "cmyk"]).is_err());
    }

    #[test]
    fn stdout_conflicts_with_json() {
        assert!(Cli::try_parse_from(["dmsshot", "--stdout", "--json"]).is_err());
    }

    #[test]
    fn gtk_and_qt_theme_flags_are_mutually_exclusive() {
        let gtk = Cli::try_parse_from(["dmsshot", "--gtk"]).unwrap();
        assert!(gtk.gtk);
        assert!(!gtk.qt);
        let qt = Cli::try_parse_from(["dmsshot", "--qt"]).unwrap();
        assert!(qt.qt);
        assert!(!qt.gtk);
        assert!(Cli::try_parse_from(["dmsshot", "--gtk", "--qt"]).is_err());
    }

    #[test]
    fn compositor_backend_can_be_forced_globally() {
        let niri = Cli::try_parse_from(["dmsshot", "--be", "niri", "full"]).unwrap();
        assert_eq!(niri.backend, Some(BackendPreference::Niri));
        let kde = Cli::try_parse_from(["dmsshot", "window", "--be", "kde"]).unwrap();
        assert_eq!(kde.backend, Some(BackendPreference::Kde));
        assert!(Cli::try_parse_from(["dmsshot", "--be", "other"]).is_err());
    }

    #[test]
    fn interval_is_bounded() {
        assert!(Cli::try_parse_from(["dmsshot", "scroll", "--interval", "29"]).is_err());
        assert!(Cli::try_parse_from(["dmsshot", "scroll", "--interval", "1001"]).is_err());
    }

    #[test]
    fn jpeg_alias_is_accepted() {
        let cli = Cli::try_parse_from(["dmsshot", "--format", "jpeg", "full"]).unwrap();
        assert_eq!(cli.format, OutputFormat::Jpg);
    }

    #[test]
    fn ocr_defaults_to_paddle_and_accepts_tesseract_options() {
        let cli = Cli::try_parse_from(["dmsshot", "ocr"]).unwrap();
        assert_eq!(
            cli.command,
            Some(Command::Ocr {
                engine: super::OcrEngine::Paddle,
                model_dir: None,
                language: "chi_sim+eng".into(),
                psm: None,
            })
        );
        let cli = Cli::try_parse_from([
            "dmsshot",
            "ocr",
            "--engine",
            "tesseract",
            "--lang",
            "chi_sim+eng",
            "--psm",
            "3",
        ])
        .unwrap();
        assert_eq!(
            cli.command,
            Some(Command::Ocr {
                engine: super::OcrEngine::Tesseract,
                model_dir: None,
                language: "chi_sim+eng".into(),
                psm: Some(3),
            })
        );
        assert!(Cli::try_parse_from(["dmsshot", "ocr", "--psm", "14"]).is_err());
    }

    #[test]
    fn notification_listener_is_hidden_but_parseable() {
        let cli = Cli::try_parse_from([
            "dmsshot",
            "notification-action",
            "/tmp/example.png",
            "--copied",
        ])
        .unwrap();
        assert_eq!(
            cli.command,
            Some(Command::NotificationAction {
                path: PathBuf::from("/tmp/example.png"),
                copied: true,
            })
        );

        let help = Cli::try_parse_from(["dmsshot", "--help"])
            .unwrap_err()
            .to_string();
        assert!(!help.contains("notification-action"));
    }
}
