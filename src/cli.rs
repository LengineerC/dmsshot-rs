use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

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
    /// Capture the focused niri window.
    Window,
    /// Select and continuously stitch a scrolling region.
    Scroll {
        /// Idle capture interval in milliseconds.
        #[arg(long, default_value_t = 45, value_parser = clap::value_parser!(u64).range(30..=1000))]
        interval: u64,
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
#[command(
    name = "dmsshot",
    version,
    about = "Native screenshot utility for niri"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Output name for output mode.
    #[arg(short = 'o', long, global = true)]
    pub output: Option<String>,

    /// Include the pointer in captured images.
    #[arg(long, value_enum, default_value_t = CursorMode::Off, global = true)]
    pub cursor: CursorMode,

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

    use super::{Cli, Command, OutputFormat};

    #[test]
    fn defaults_to_region() {
        let cli = Cli::try_parse_from(["dmsshot"]).unwrap();
        assert_eq!(cli.command(), Command::Region);
        assert_eq!(cli.format, OutputFormat::Png);
    }

    #[test]
    fn stdout_conflicts_with_json() {
        assert!(Cli::try_parse_from(["dmsshot", "--stdout", "--json"]).is_err());
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
