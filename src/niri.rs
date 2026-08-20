use std::{
    collections::BTreeMap,
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

#[derive(Debug, Clone, PartialEq)]
pub struct OutputInfo {
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

#[derive(Debug, Deserialize)]
struct WireOutput {
    name: String,
    modes: Vec<WireMode>,
    current_mode: Option<usize>,
    logical: Option<WireLogicalOutput>,
}

#[derive(Debug, Deserialize)]
struct WireMode {
    width: u32,
    height: u32,
}

#[derive(Debug, Deserialize)]
struct WireLogicalOutput {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale: f64,
    transform: WireTransform,
}

#[derive(Debug, Deserialize)]
enum WireTransform {
    Normal,
    #[serde(rename = "90")]
    Rotate90,
    #[serde(rename = "180")]
    Rotate180,
    #[serde(rename = "270")]
    Rotate270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl WireTransform {
    fn code(&self) -> i32 {
        match self {
            Self::Normal => 0,
            Self::Rotate90 => 1,
            Self::Rotate180 => 2,
            Self::Rotate270 => 3,
            Self::Flipped => 4,
            Self::Flipped90 => 5,
            Self::Flipped180 => 6,
            Self::Flipped270 => 7,
        }
    }
}

#[derive(Debug, Deserialize)]
struct FocusedOutput {
    name: String,
}

pub fn ensure_niri() -> Result<()> {
    socket_path().map(|_| ())
}

pub fn outputs() -> Result<Vec<OutputInfo>> {
    let value = request(json!("Outputs"))?;
    let outputs = response_variant(value, "Outputs")?;
    let wire: BTreeMap<String, WireOutput> = serde_json::from_value(outputs)?;
    let mut result = Vec::new();
    for (_, output) in wire {
        let Some(logical) = output.logical else {
            continue;
        };
        let Some(mode) = output
            .current_mode
            .and_then(|index| output.modes.get(index))
        else {
            continue;
        };
        result.push(OutputInfo {
            name: output.name,
            physical_width: mode.width,
            physical_height: mode.height,
            x: logical.x,
            y: logical.y,
            logical_width: logical.width,
            logical_height: logical.height,
            scale: logical.scale,
            transform: logical.transform.code(),
        });
    }
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}

pub fn focused_output_name() -> Result<Option<String>> {
    let value = request(json!("FocusedOutput"))?;
    let value = response_variant(value, "FocusedOutput")?;
    let output: Option<FocusedOutput> = serde_json::from_value(value)?;
    Ok(output.map(|output| output.name))
}

pub fn capture_focused_window(show_pointer: bool) -> Result<ImageBuffer> {
    let path = temporary_screenshot_path();
    let result = capture_focused_window_to(&path, show_pointer);
    let _ = std::fs::remove_file(&path);
    result
}

fn capture_focused_window_to(path: &PathBuf, show_pointer: bool) -> Result<ImageBuffer> {
    let socket = socket_path()?;
    let events_stream = UnixStream::connect(&socket)?;
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
    let reply = request(action)?;
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
        let captured_path = captured.get("path").and_then(Value::as_str);
        if captured_path != path.to_str() {
            continue;
        }
        break;
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

fn request(value: Value) -> Result<Value> {
    let mut stream = UnixStream::connect(socket_path()?)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    write_json_line(&mut stream, &value)?;
    read_json_line(&mut BufReader::new(stream))
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

fn socket_path() -> Result<PathBuf> {
    env::var_os("NIRI_SOCKET")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or(Error::NotNiri)
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
    fn parses_outputs_reply() {
        let value = json!({"Ok":{"Outputs":{"DP-1":{
            "name":"DP-1","modes":[{"width":2560,"height":1440}],"current_mode":0,
            "logical":{"x":-100,"y":0,"width":2048,"height":1152,"scale":1.25,"transform":"Normal"}
        }}}});
        let body = response_variant(value, "Outputs").unwrap();
        let parsed: BTreeMap<String, WireOutput> = serde_json::from_value(body).unwrap();
        assert_eq!(parsed["DP-1"].logical.as_ref().unwrap().scale, 1.25);
    }

    #[test]
    fn reports_niri_error() {
        let err = response_variant(json!({"Err":"no focused window"}), "Handled").unwrap_err();
        assert!(err.to_string().contains("no focused window"));
    }
}
