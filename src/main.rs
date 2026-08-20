mod app;
mod cli;
mod error;
mod imagebuf;
mod integration;
mod niri;
mod overlay;
mod state;
mod stitch;
mod wayland;

use std::process::ExitCode;

use clap::Parser;

use crate::{
    app::Application,
    cli::{Cli, Command},
};

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(Command::NotificationAction { path, copied }) = cli.command.as_ref() {
        integration::run_notification_listener(path, *copied);
        return ExitCode::SUCCESS;
    }
    let json = cli.json;
    match Application::new(cli).and_then(Application::run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            if json {
                app::print_json_error(&err);
            } else {
                eprintln!("Error: {err}");
            }
            ExitCode::FAILURE
        }
    }
}
