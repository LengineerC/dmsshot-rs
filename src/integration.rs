use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use std::os::unix::process::CommandExt;

use crate::error::{Error, Result};

pub fn copy_image(data: &[u8], mime: &str) -> Result<()> {
    copy_to_clipboard(data, mime)
}

pub fn copy_text(text: &str) -> Result<()> {
    copy_to_clipboard(text.as_bytes(), "text/plain;charset=utf-8")
}

fn copy_to_clipboard(data: &[u8], mime: &str) -> Result<()> {
    let mut child = Command::new("wl-copy")
        .args(["--type", mime])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        // wl-copy forks a background owner. Capturing stderr here would keep the pipe open in
        // that owner and make wait_with_output block for the whole clipboard lifetime.
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| Error::Clipboard(format!("unable to start wl-copy: {error}")))?;

    child
        .stdin
        .take()
        .ok_or_else(|| Error::Clipboard("wl-copy stdin is unavailable".into()))?
        .write_all(data)
        .map_err(|error| Error::Clipboard(format!("unable to send data to wl-copy: {error}")))?;

    let status = child
        .wait()
        .map_err(|error| Error::Clipboard(format!("unable to wait for wl-copy: {error}")))?;
    if status.success() {
        return Ok(());
    }
    Err(Error::Clipboard(format!("wl-copy exited with {status}")))
}

pub fn notify_ocr_success(copied: bool) {
    let body = if copied {
        "Recognized text copied to clipboard"
    } else {
        "Text recognition completed"
    };
    let _ = Command::new("notify-send")
        .args([
            "--app-name",
            "dmsshot-rs",
            "--expire-time",
            "5000",
            "Text recognized",
            body,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub fn notify_success(path: &Path, copied: bool) {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if spawn_notification_listener(&path, copied).is_err() {
        send_plain_notification(&path, copied);
    }
}

fn spawn_notification_listener(path: &Path, copied: bool) -> std::io::Result<()> {
    let mut command = Command::new(std::env::current_exe()?);
    command.arg("notification-action").arg(path);
    if copied {
        command.arg("--copied");
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The screenshot process exits immediately after sending the notification. A new session keeps
    // the listener alive when dmsshot was launched from a terminal or a compositor key binding.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    command.spawn().map(|_| ())
}

pub fn run_notification_listener(path: &Path, copied: bool) {
    let body = notification_body(path, copied);
    let output = Command::new("notify-send")
        .args([
            "--app-name",
            "dmsshot-rs",
            "--expire-time",
            "5000",
            "--action=default=Open",
            "--action=folder=Open Folder",
            "--icon",
        ])
        .arg(path)
        .arg("Screenshot captured")
        .arg(body)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(output) = output else {
        return;
    };
    let action = String::from_utf8_lossy(&output.stdout);
    let Some(target) = notification_action_target(action.trim(), path) else {
        return;
    };
    let _ = Command::new("xdg-open")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn send_plain_notification(path: &Path, copied: bool) {
    let body = notification_body(path, copied);
    let _ = Command::new("notify-send")
        .args([
            "--app-name",
            "dmsshot-rs",
            "--expire-time",
            "5000",
            "--icon",
        ])
        .arg(path)
        .arg("Screenshot captured")
        .arg(body)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn notification_body(path: &Path, copied: bool) -> String {
    let filename = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into(),
    );
    if copied {
        format!("{filename}\nCopied to clipboard")
    } else {
        filename
    }
}

fn notification_action_target(action: &str, path: &Path) -> Option<PathBuf> {
    match action {
        "default" | "open" => Some(path.to_path_buf()),
        "folder" => path.parent().map(Path::to_path_buf),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_uses_only_the_file_name() {
        let path = Path::new("/tmp/example.png");
        assert_eq!(notification_body(path, false), "example.png");
        assert_eq!(
            notification_body(path, true),
            "example.png\nCopied to clipboard"
        );
    }

    #[test]
    fn notification_actions_resolve_to_file_and_folder() {
        let path = Path::new("/tmp/screenshots/example.png");
        assert_eq!(
            notification_action_target("default", path),
            Some(path.to_path_buf())
        );
        assert_eq!(
            notification_action_target("open", path),
            Some(path.to_path_buf())
        );
        assert_eq!(
            notification_action_target("folder", path),
            Some(PathBuf::from("/tmp/screenshots"))
        );
        assert_eq!(notification_action_target("", path), None);
    }
}
