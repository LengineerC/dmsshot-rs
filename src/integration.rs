use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use crate::error::{Error, Result};

pub fn copy_image(data: &[u8], mime: &str) -> Result<()> {
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
        .map_err(|error| Error::Clipboard(format!("unable to send image to wl-copy: {error}")))?;

    let status = child
        .wait()
        .map_err(|error| Error::Clipboard(format!("unable to wait for wl-copy: {error}")))?;
    if status.success() {
        return Ok(());
    }
    Err(Error::Clipboard(format!("wl-copy exited with {status}")))
}

pub fn notify_success(path: &Path, copied: bool) {
    let filename = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into(),
    );
    let body = if copied {
        format!("{filename}\nCopied to clipboard")
    } else {
        filename
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_uses_only_the_file_name() {
        let path = Path::new("/tmp/example.png");
        assert_eq!(path.file_name().unwrap().to_string_lossy(), "example.png");
    }
}
