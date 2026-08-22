use std::{
    collections::HashMap, io::Read, os::fd::AsFd, os::unix::net::UnixStream, time::Duration,
};

use zbus::{
    blocking::{Connection, Proxy},
    zvariant::{Fd, OwnedValue, Value},
};

use crate::{
    backend::OutputInfo,
    error::{Error, Result},
    imagebuf::{ImageBuffer, Region},
};

const SERVICE: &str = "org.kde.KWin.ScreenShot2";
const PATH: &str = "/org/kde/KWin/ScreenShot2";
const INTERFACE: &str = "org.kde.KWin.ScreenShot2";
const PIPE_TIMEOUT: Duration = Duration::from_secs(10);
const AUTHORIZATION_HINT: &str = "install the root-owned /usr/share/applications/dmsshot-rs.desktop with Exec=/usr/bin/dmsshot-rs and X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2, then run /usr/bin/dmsshot-rs instead of an unpackaged copy";

// Stable QImage::Format values used by KWin's ScreenShot2 interface.
const QIMAGE_RGB32: u32 = 4;
const QIMAGE_ARGB32: u32 = 5;
const QIMAGE_ARGB32_PREMULTIPLIED: u32 = 6;
const QIMAGE_RGB888: u32 = 13;
const QIMAGE_RGBA8888: u32 = 17;
const QIMAGE_RGBA8888_PREMULTIPLIED: u32 = 18;
const QIMAGE_BGR888: u32 = 29;

type Results = HashMap<String, OwnedValue>;
type Options<'a> = HashMap<&'a str, Value<'a>>;

#[derive(Clone)]
pub struct KdeBackend {
    connection: Connection,
    version: u32,
}

impl KdeBackend {
    pub fn detect() -> Option<Self> {
        let connection = Connection::session().ok()?;
        let proxy = screenshot_proxy(&connection).ok()?;
        let version = proxy.get_property::<u32>("Version").ok()?;
        drop(proxy);
        Some(Self {
            connection,
            version,
        })
    }

    pub fn capture_active_window(&self, show_pointer: bool) -> Result<ImageBuffer> {
        if self.version < 2 {
            return Err(Error::Kde(
                "CaptureActiveWindow requires ScreenShot2 version 2 or newer".into(),
            ));
        }
        let (mut reader, writer) = screenshot_pipe()?;
        let options = window_options(show_pointer);
        let fd = Fd::from(writer.as_fd());
        let results: Results = self
            .proxy()?
            .call("CaptureActiveWindow", &(&options, fd))
            .map_err(dbus_error)?;
        drop(writer);
        let mut image = read_image(&mut reader, &results)?;
        image.scale = result_f64(&results, "scale").unwrap_or(1.0);
        image.region = Region {
            width: image.width as i32,
            height: image.height as i32,
            ..Region::default()
        };
        Ok(image)
    }

    pub fn capture_screen(&self, output: &OutputInfo, show_pointer: bool) -> Result<ImageBuffer> {
        let (mut reader, writer) = screenshot_pipe()?;
        let options = screen_options(show_pointer);
        let fd = Fd::from(writer.as_fd());
        let results: Results = self
            .proxy()?
            .call("CaptureScreen", &(&output.name, &options, fd))
            .map_err(dbus_error)?;
        drop(writer);
        let mut image = read_image(&mut reader, &results)?;
        set_output_metadata(&mut image, output, &results);
        Ok(image)
    }

    pub fn capture_active_screen(
        &self,
        outputs: &[OutputInfo],
        show_pointer: bool,
    ) -> Result<ImageBuffer> {
        if self.version < 2 {
            return Err(Error::Kde(
                "CaptureActiveScreen requires ScreenShot2 version 2 or newer".into(),
            ));
        }
        let (mut reader, writer) = screenshot_pipe()?;
        let options = screen_options(show_pointer);
        let fd = Fd::from(writer.as_fd());
        let results: Results = self
            .proxy()?
            .call("CaptureActiveScreen", &(&options, fd))
            .map_err(dbus_error)?;
        drop(writer);
        let mut image = read_image(&mut reader, &results)?;
        let output = result_string(&results, "screen")
            .and_then(|name| outputs.iter().find(|output| output.name == name))
            .or_else(|| outputs.first())
            .ok_or(Error::NoOutputs)?;
        set_output_metadata(&mut image, output, &results);
        Ok(image)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn capture_area(
        &self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        output: &OutputInfo,
        show_pointer: bool,
    ) -> Result<ImageBuffer> {
        if width <= 0 || height <= 0 {
            return Err(Error::InvalidBuffer("empty capture region".into()));
        }
        if self.version < 5 {
            return Err(Error::Kde(
                "scroll capture requires KWin ScreenShot2 version 5 so the overlay can be hidden"
                    .into(),
            ));
        }
        let (mut reader, writer) = screenshot_pipe()?;
        let options = screen_options(show_pointer);
        let fd = Fd::from(writer.as_fd());
        let results: Results = self
            .proxy()?
            .call(
                "CaptureArea",
                &(x, y, width as u32, height as u32, &options, fd),
            )
            .map_err(dbus_error)?;
        drop(writer);
        let mut image = read_image(&mut reader, &results)?;
        image.scale = result_f64(&results, "scale").unwrap_or(output.scale);
        image.region = Region {
            x,
            y,
            width,
            height,
            output: output.name.clone(),
        };
        Ok(image)
    }

    pub fn capture_workspace(
        &self,
        outputs: &[OutputInfo],
        show_pointer: bool,
    ) -> Result<ImageBuffer> {
        if self.version < 3 {
            return Err(Error::Kde(
                "CaptureWorkspace requires ScreenShot2 version 3 or newer".into(),
            ));
        }
        let (mut reader, writer) = screenshot_pipe()?;
        let options = screen_options(show_pointer);
        let fd = Fd::from(writer.as_fd());
        let results: Results = self
            .proxy()?
            .call("CaptureWorkspace", &(&options, fd))
            .map_err(dbus_error)?;
        drop(writer);
        let mut image = read_image(&mut reader, &results)?;
        let min_x = outputs.iter().map(|output| output.x).min().unwrap_or(0);
        let min_y = outputs.iter().map(|output| output.y).min().unwrap_or(0);
        image.scale = result_f64(&results, "scale").unwrap_or(1.0);
        image.region = Region {
            x: min_x,
            y: min_y,
            width: image.width as i32,
            height: image.height as i32,
            ..Region::default()
        };
        Ok(image)
    }

    fn proxy(&self) -> Result<Proxy<'_>> {
        screenshot_proxy(&self.connection).map_err(dbus_error)
    }
}

fn screenshot_proxy(connection: &Connection) -> zbus::Result<Proxy<'_>> {
    Proxy::new(connection, SERVICE, PATH, INTERFACE)
}

fn screenshot_pipe() -> Result<(UnixStream, UnixStream)> {
    let (reader, writer) = UnixStream::pair()?;
    reader.set_read_timeout(Some(PIPE_TIMEOUT))?;
    Ok((reader, writer))
}

fn screen_options(show_pointer: bool) -> Options<'static> {
    HashMap::from([
        ("include-cursor", Value::from(show_pointer)),
        ("native-resolution", Value::from(true)),
        ("hide-caller-windows", Value::from(true)),
    ])
}

fn window_options(show_pointer: bool) -> Options<'static> {
    HashMap::from([
        ("include-cursor", Value::from(show_pointer)),
        ("include-decoration", Value::from(true)),
        ("include-shadow", Value::from(true)),
        ("native-resolution", Value::from(true)),
    ])
}

fn read_image(reader: &mut UnixStream, results: &Results) -> Result<ImageBuffer> {
    let image_type = result_string(results, "type")
        .ok_or_else(|| Error::Kde("reply did not contain an image type".into()))?;
    if image_type != "raw" {
        return Err(Error::Kde(format!(
            "unsupported ScreenShot2 image type {image_type:?}"
        )));
    }
    let width = result_u32(results, "width")?;
    let height = result_u32(results, "height")?;
    let stride = result_u32(results, "stride")?;
    let format = result_u32(results, "format")?;
    let size = (stride as usize)
        .checked_mul(height as usize)
        .ok_or(Error::ImageTooLarge)?;
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes).map_err(|error| {
        Error::Kde(format!(
            "failed to read {size} bytes of screenshot pixels: {error}"
        ))
    })?;
    qimage_to_rgba(width, height, stride, format, &bytes)
}

fn qimage_to_rgba(
    width: u32,
    height: u32,
    stride: u32,
    format: u32,
    source: &[u8],
) -> Result<ImageBuffer> {
    let bytes_per_pixel = match format {
        QIMAGE_RGB888 | QIMAGE_BGR888 => 3,
        QIMAGE_RGB32
        | QIMAGE_ARGB32
        | QIMAGE_ARGB32_PREMULTIPLIED
        | QIMAGE_RGBA8888
        | QIMAGE_RGBA8888_PREMULTIPLIED => 4,
        _ => {
            return Err(Error::InvalidBuffer(format!(
                "unsupported QImage format {format} from KWin"
            )));
        }
    };
    let minimum = width
        .checked_mul(bytes_per_pixel)
        .ok_or(Error::ImageTooLarge)?;
    if stride < minimum {
        return Err(Error::InvalidBuffer(format!(
            "KWin stride {stride} is smaller than {width}x{bytes_per_pixel}"
        )));
    }
    let expected = (stride as usize)
        .checked_mul(height as usize)
        .ok_or(Error::ImageTooLarge)?;
    if source.len() < expected {
        return Err(Error::InvalidBuffer(format!(
            "KWin buffer has {} bytes, expected {expected}",
            source.len()
        )));
    }

    let mut image = ImageBuffer::new(width, height)?;
    for y in 0..height as usize {
        for x in 0..width as usize {
            let source_index = y * stride as usize + x * bytes_per_pixel as usize;
            let destination = (y * width as usize + x) * 4;
            let (mut red, mut green, mut blue, alpha, premultiplied) = match format {
                QIMAGE_RGB888 => (
                    source[source_index],
                    source[source_index + 1],
                    source[source_index + 2],
                    255,
                    false,
                ),
                QIMAGE_BGR888 => (
                    source[source_index + 2],
                    source[source_index + 1],
                    source[source_index],
                    255,
                    false,
                ),
                QIMAGE_RGBA8888 | QIMAGE_RGBA8888_PREMULTIPLIED => (
                    source[source_index],
                    source[source_index + 1],
                    source[source_index + 2],
                    source[source_index + 3],
                    format == QIMAGE_RGBA8888_PREMULTIPLIED,
                ),
                QIMAGE_RGB32 | QIMAGE_ARGB32 | QIMAGE_ARGB32_PREMULTIPLIED => {
                    let (r, g, b, a) = argb32_channels(&source[source_index..source_index + 4]);
                    (
                        r,
                        g,
                        b,
                        if format == QIMAGE_RGB32 { 255 } else { a },
                        format == QIMAGE_ARGB32_PREMULTIPLIED,
                    )
                }
                _ => unreachable!(),
            };
            if premultiplied && alpha != 255 {
                red = unpremultiply(red, alpha);
                green = unpremultiply(green, alpha);
                blue = unpremultiply(blue, alpha);
            }
            image.pixels[destination..destination + 4].copy_from_slice(&[red, green, blue, alpha]);
        }
    }
    Ok(image)
}

#[cfg(target_endian = "little")]
fn argb32_channels(pixel: &[u8]) -> (u8, u8, u8, u8) {
    (pixel[2], pixel[1], pixel[0], pixel[3])
}

#[cfg(target_endian = "big")]
fn argb32_channels(pixel: &[u8]) -> (u8, u8, u8, u8) {
    (pixel[1], pixel[2], pixel[3], pixel[0])
}

fn unpremultiply(channel: u8, alpha: u8) -> u8 {
    if alpha == 0 {
        0
    } else {
        ((u32::from(channel) * 255 + u32::from(alpha) / 2) / u32::from(alpha)).min(255) as u8
    }
}

fn set_output_metadata(image: &mut ImageBuffer, output: &OutputInfo, results: &Results) {
    image.scale = result_f64(results, "scale").unwrap_or(output.scale);
    image.region = Region {
        x: output.x,
        y: output.y,
        width: image.width as i32,
        height: image.height as i32,
        output: output.name.clone(),
    };
}

fn result_u32(results: &Results, key: &str) -> Result<u32> {
    results
        .get(key)
        .ok_or_else(|| Error::Kde(format!("reply did not contain {key:?}")))?
        .downcast_ref::<u32>()
        .map_err(|error| Error::Kde(format!("invalid {key:?} result: {error}")))
}

fn result_f64(results: &Results, key: &str) -> Option<f64> {
    results.get(key)?.downcast_ref::<f64>().ok()
}

fn result_string<'a>(results: &'a Results, key: &str) -> Option<&'a str> {
    results.get(key)?.downcast_ref::<&str>().ok()
}

fn dbus_error(error: zbus::Error) -> Error {
    let message = error.to_string();
    if message.to_ascii_lowercase().contains("not authorized") {
        Error::Kde(format!("{message}; {AUTHORIZATION_HINT}"))
    } else {
        Error::Kde(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_file_authorizes_the_installed_executable() {
        let desktop = include_str!("../../data/dmsshot-rs.desktop");
        assert!(desktop.contains("Exec=/usr/bin/dmsshot-rs\n"));
        assert!(desktop.contains(&format!("X-KDE-DBUS-Restricted-Interfaces={INTERFACE}\n")));
        assert!(AUTHORIZATION_HINT.contains("/usr/bin/dmsshot-rs"));
    }

    #[test]
    fn converts_kwin_rgba_with_stride() {
        let source = [10, 20, 30, 255, 40, 50, 60, 128, 0, 0, 0, 0];
        let image = qimage_to_rgba(2, 1, 12, QIMAGE_RGBA8888, &source).unwrap();
        assert_eq!(image.pixels, vec![10, 20, 30, 255, 40, 50, 60, 128]);
    }

    #[test]
    fn unpremultiplies_kwin_window_pixels() {
        let source = [50, 25, 10, 128];
        let image = qimage_to_rgba(1, 1, 4, QIMAGE_RGBA8888_PREMULTIPLIED, &source).unwrap();
        assert_eq!(image.pixels, vec![100, 50, 20, 128]);
    }

    #[test]
    fn converts_qimage_argb32_memory_order() {
        #[cfg(target_endian = "little")]
        let source = [30, 20, 10, 128];
        #[cfg(target_endian = "big")]
        let source = [128, 10, 20, 30];
        let image = qimage_to_rgba(1, 1, 4, QIMAGE_ARGB32, &source).unwrap();
        assert_eq!(image.pixels, vec![10, 20, 30, 128]);
    }
}
