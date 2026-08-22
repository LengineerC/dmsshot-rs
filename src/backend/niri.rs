use std::{
    env,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    error::{Error, Result},
    imagebuf::{ImageBuffer, Region},
};

const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Deserialize)]
struct FocusedOutput {
    name: String,
}

pub struct NiriBackend {
    socket: PathBuf,
}

impl NiriBackend {
    pub fn detect() -> Option<Self> {
        env::var_os("NIRI_SOCKET")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|socket| Self { socket })
    }

    pub fn focused_output_name(&self) -> Result<Option<String>> {
        let value = self.request(json!("FocusedOutput"))?;
        let value = response_variant(value, "FocusedOutput")?;
        let output: Option<FocusedOutput> = serde_json::from_value(value)?;
        Ok(output.map(|output| output.name))
    }

    pub fn capture_focused_window(&self, show_pointer: bool) -> Result<ImageBuffer> {
        let path = temporary_screenshot_path();
        let result = self.capture_focused_window_to(&path, show_pointer);
        let _ = std::fs::remove_file(&path);
        result
    }

    fn capture_focused_window_to(&self, path: &PathBuf, show_pointer: bool) -> Result<ImageBuffer> {
        let events_stream = UnixStream::connect(&self.socket)?;
        events_stream.set_read_timeout(Some(SCREENSHOT_TIMEOUT))?;
        let mut events = BufReader::new(events_stream);
        write_json_line(events.get_mut(), &json!("EventStream"))?;
        let reply = read_json_line(&mut events)?;
        response_variant(reply, "Handled")?;

        let action = json!({
            "Action": {
                "ScreenshotWindow": {
                    "id": null,
                    "write_to_disk": true,
                    "show_pointer": show_pointer,
                    "path": path.to_string_lossy()
                }
            }
        });
        let reply = self.request(action)?;
        response_variant(reply, "Handled")?;

        loop {
            let event = read_json_line(&mut events).map_err(|err| match err {
                Error::Io(io)
                    if matches!(
                        io.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    Error::Niri("timed out waiting for ScreenshotCaptured".into())
                }
                other => other,
            })?;
            let Some(captured) = event.get("ScreenshotCaptured") else {
                continue;
            };
            if captured.get("path").and_then(Value::as_str) == path.to_str() {
                break;
            }
        }

        let rgba = image::open(path)?.to_rgba8();
        let (width, height) = rgba.dimensions();
        Ok(ImageBuffer {
            width,
            height,
            pixels: rgba.into_raw(),
            scale: 1.0,
            region: Region {
                width: width as i32,
                height: height as i32,
                ..Region::default()
            },
        })
    }

    fn request(&self, value: Value) -> Result<Value> {
        let mut stream = UnixStream::connect(&self.socket)?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        write_json_line(&mut stream, &value)?;
        read_json_line(&mut BufReader::new(stream))
    }
}

fn response_variant(value: Value, expected: &str) -> Result<Value> {
    let object = value
        .as_object()
        .ok_or_else(|| Error::Niri(format!("invalid reply: {value}")))?;
    if let Some(message) = object.get("Err") {
        return Err(Error::Niri(
            message.as_str().unwrap_or("unknown niri error").to_owned(),
        ));
    }
    let ok = object
        .get("Ok")
        .ok_or_else(|| Error::Niri(format!("invalid reply: {value}")))?;
    if expected == "Handled" && ok == "Handled" {
        return Ok(Value::Null);
    }
    ok.get(expected)
        .cloned()
        .ok_or_else(|| Error::Niri(format!("expected {expected} response, received {ok}")))
}

fn write_json_line(stream: &mut UnixStream, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *stream, value)?;
    stream.write_all(b"\n")?;
    Ok(())
}

fn read_json_line(reader: &mut BufReader<UnixStream>) -> Result<Value> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Err(Error::Niri("IPC socket closed before a reply".into()));
    }
    Ok(serde_json::from_str(&line)?)
}

fn temporary_screenshot_path() -> PathBuf {
    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    runtime.join(format!("dmsshot-window-{}-{nanos}.png", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_niri_error() {
        let err = response_variant(json!({"Err":"no focused window"}), "Handled").unwrap_err();
        assert!(err.to_string().contains("no focused window"));
    }
}
