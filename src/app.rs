use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    time::Duration,
};

use serde::Serialize;

use crate::{
    backend::{CaptureSource, DesktopBackend, OutputInfo, WindowCapture},
    cli::{Cli, Command, OcrEngine},
    error::{Error, Result},
    imagebuf::ImageBuffer,
    integration, ocr,
    overlay::{self, Background, SelectorOptions, ThemeSource},
    state,
    wayland::WaylandCapture,
};

pub struct Application {
    cli: Cli,
    backend: DesktopBackend,
}

#[derive(Serialize)]
struct Metadata<'a> {
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scale: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mime: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

#[derive(Serialize)]
struct OcrMetadata<'a> {
    status: &'a str,
    text: &'a str,
    width: u32,
    height: u32,
    engine: &'a str,
    language: &'a str,
}

impl Application {
    pub fn new(cli: Cli) -> Self {
        let backend = DesktopBackend::detect(cli.backend);
        Self { cli, backend }
    }

    pub fn run(self) -> Result<()> {
        let command = self.cli.command();
        if command == Command::List {
            return self.list_outputs();
        }
        if let Command::Ocr {
            engine,
            model_dir,
            language,
            psm,
        } = &command
        {
            return self.run_ocr(*engine, model_dir.as_deref(), language, *psm);
        }

        let mut clipboard_already_set = false;

        let image = match command {
            Command::Window => {
                let captured = self.capture_window()?;
                clipboard_already_set = captured.clipboard_already_set;
                Some(captured.image)
            }
            Command::Full => Some(self.capture_full()?),
            Command::All => Some(self.capture_all()?),
            Command::Output { name } => {
                let name = self
                    .cli
                    .output_name(name.as_deref())
                    .ok_or(Error::MissingOutputName)?;
                Some(self.capture_named(&name)?)
            }
            Command::Region => self.capture_interactive(false, 45)?,
            Command::Scroll { interval } => self.capture_interactive(true, interval)?,
            Command::Ocr { .. } => unreachable!(),
            Command::List => unreachable!(),
            Command::NotificationAction { .. } => unreachable!(),
        };

        let Some(image) = image else {
            if self.cli.json {
                print_json(&Metadata {
                    status: "aborted",
                    path: None,
                    width: None,
                    height: None,
                    scale: None,
                    mime: None,
                    error: Some("User cancelled selection"),
                })?;
            }
            return Ok(());
        };
        self.write_result(&image, clipboard_already_set)
    }

    fn run_ocr(
        &self,
        engine: OcrEngine,
        model_dir: Option<&std::path::Path>,
        language: &str,
        psm: Option<u8>,
    ) -> Result<()> {
        let paddle_models = ocr::ensure_available(engine, model_dir)?;
        let Some(image) = self.capture_interactive(false, 45)? else {
            if self.cli.json {
                print_json(&Metadata {
                    status: "aborted",
                    path: None,
                    width: None,
                    height: None,
                    scale: None,
                    mime: None,
                    error: Some("User cancelled selection"),
                })?;
            }
            return Ok(());
        };
        let text = ocr::recognize(engine, paddle_models.as_ref(), &image, language, psm)?;
        let copied = !self.cli.no_clipboard;
        if copied {
            integration::copy_text(&text)?;
        }
        if self.cli.json {
            print_json(&OcrMetadata {
                status: "success",
                text: &text,
                width: image.width,
                height: image.height,
                engine: engine.name(),
                language: if engine == OcrEngine::Paddle {
                    "ch+en"
                } else {
                    language
                },
            })?;
        } else {
            println!("{text}");
        }
        if !self.cli.no_notify {
            integration::notify_ocr_success(copied);
        }
        Ok(())
    }

    fn list_outputs(&self) -> Result<()> {
        let capture = WaylandCapture::connect()?;
        for output in capture.outputs() {
            println!(
                "{}: {}x{}+{}+{} scale={} transform={}",
                output.name,
                output.physical_width,
                output.physical_height,
                output.x,
                output.y,
                format_scale(output.scale),
                transform_name(output.transform),
            );
        }
        Ok(())
    }

    fn capture_window(&self) -> Result<WindowCapture> {
        let mut captured = self
            .backend
            .capture_focused_window(self.cli.cursor.enabled())?;
        if let Some(name) = self.backend.focused_output_name()
            && let Ok(capture) = WaylandCapture::connect()
            && let Some(output) = capture
                .outputs()
                .into_iter()
                .find(|output| output.name == name)
        {
            captured.image.scale = output.scale;
            captured.image.region.output = output.name;
        }
        Ok(captured)
    }

    fn capture_full(&self) -> Result<ImageBuffer> {
        let mut capture = self.backend.capture_session()?;
        if let Some(image) = capture.capture_active_output(self.cli.cursor.enabled())? {
            return Ok(image);
        }
        let outputs = capture.outputs();
        let focused = self.backend.focused_output_name();
        let output = select_full_output(&outputs, focused.as_deref()).ok_or(Error::NoOutputs)?;
        capture.capture_output(output, self.cli.cursor.enabled())
    }

    fn capture_named(&self, name: &str) -> Result<ImageBuffer> {
        let mut capture = self.backend.capture_session()?;
        let output = capture
            .outputs()
            .into_iter()
            .find(|output| output.name == name)
            .ok_or_else(|| Error::OutputNotFound(name.to_owned()))?;
        capture.capture_output(&output, self.cli.cursor.enabled())
    }

    fn capture_all(&self) -> Result<ImageBuffer> {
        let mut capture = self.backend.capture_session()?;
        if let Some(image) = capture.capture_workspace(self.cli.cursor.enabled())? {
            return Ok(image);
        }
        let outputs = capture.outputs();
        if outputs.is_empty() {
            return Err(Error::NoOutputs);
        }
        let mut captured = Vec::with_capacity(outputs.len());
        for output in outputs {
            captured.push((
                capture.capture_output(&output, self.cli.cursor.enabled())?,
                output,
            ));
        }
        compose_outputs(captured)
    }

    fn capture_interactive(&self, scroll: bool, interval_ms: u64) -> Result<Option<ImageBuffer>> {
        if self.cli.reset {
            state::reset_last_region()?;
        }
        let mut capture = self.backend.capture_session()?;
        let outputs = capture.outputs();
        if outputs.is_empty() {
            return Err(Error::NoOutputs);
        }
        let mut backgrounds = Vec::with_capacity(outputs.len());
        for info in outputs {
            let with_cursor = capture.capture_output(&info, true)?;
            let without_cursor = capture.capture_output(&info, false)?;
            backgrounds.push(Background {
                info,
                with_cursor,
                without_cursor,
            });
        }
        let mut image = overlay::select_region(
            backgrounds,
            SelectorOptions {
                no_confirm: self.cli.no_confirm,
                scroll,
                interval: Duration::from_millis(interval_ms),
                initial: state::load_last_region(),
                show_cursor: self.cli.cursor.enabled(),
                magnifier: self.cli.magnifier,
                theme_source: if self.cli.qt {
                    ThemeSource::Qt
                } else {
                    ThemeSource::Gtk
                },
            },
            &mut capture,
        )?;
        if let Some(image) = &mut image {
            state::save_last_region(image.region.clone())?;
        }
        Ok(image)
    }

    fn write_result(&self, image: &ImageBuffer, clipboard_already_set: bool) -> Result<()> {
        if self.cli.stdout {
            let stdout = io::stdout();
            let mut lock = stdout.lock();
            image.write_encoded(&mut lock, self.cli.format, self.cli.quality)?;
            lock.flush()?;
            return Ok(());
        }

        let mut encoded = Vec::new();
        image.write_encoded(&mut encoded, self.cli.format, self.cli.quality)?;

        let directory = state::output_directory(self.cli.output_dir.as_deref())?;
        let filename = self
            .cli
            .filename
            .clone()
            .unwrap_or_else(|| PathBuf::from(state::generated_filename(self.cli.format)));
        let path = directory.join(filename);
        fs::write(&path, &encoded).map_err(|source| Error::WriteFile {
            path: path.clone(),
            source,
        })?;

        let copied = clipboard_already_set || !self.cli.no_clipboard;
        if !clipboard_already_set && copied {
            integration::copy_image(&encoded, self.cli.format.mime())?;
        }

        let path_text = path.to_string_lossy();
        if self.cli.json {
            print_json(&Metadata {
                status: "success",
                path: Some(&path_text),
                width: Some(image.width),
                height: Some(image.height),
                scale: Some(if image.scale > 0.0 { image.scale } else { 1.0 }),
                mime: Some(self.cli.format.mime()),
                error: None,
            })?;
        } else {
            println!("{path_text}");
        }
        if !self.cli.no_notify {
            integration::notify_success(&path, copied);
        }
        Ok(())
    }
}

pub fn print_json_error(error: &Error) {
    let _ = print_json(&Metadata {
        status: "error",
        path: None,
        width: None,
        height: None,
        scale: None,
        mime: None,
        error: Some(&error.to_string()),
    });
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    serde_json::to_writer(io::stdout().lock(), value)?;
    println!();
    Ok(())
}

fn compose_outputs(captured: Vec<(ImageBuffer, OutputInfo)>) -> Result<ImageBuffer> {
    if captured.is_empty() {
        return Err(Error::NoOutputs);
    }
    if captured.len() == 1 {
        return Ok(captured.into_iter().next().unwrap().0);
    }
    let max_scale = captured
        .iter()
        .map(|(_, output)| output.scale.max(1.0))
        .fold(1.0f64, f64::max);
    let entries: Vec<_> = captured
        .iter()
        .map(|(image, output)| {
            let x = (output.x as f64 * max_scale).round() as i32;
            let y = (output.y as f64 * max_scale).round() as i32;
            let width = (output.logical_width as f64 * max_scale).round() as u32;
            let height = (output.logical_height as f64 * max_scale).round() as u32;
            (image, x, y, width, height)
        })
        .collect();
    let min_x = entries.iter().map(|(_, x, _, _, _)| *x).min().unwrap();
    let min_y = entries.iter().map(|(_, _, y, _, _)| *y).min().unwrap();
    let max_x = entries
        .iter()
        .map(|(_, x, _, width, _)| *x + *width as i32)
        .max()
        .unwrap();
    let max_y = entries
        .iter()
        .map(|(_, _, y, _, height)| *y + *height as i32)
        .max()
        .unwrap();
    let mut canvas = ImageBuffer::new((max_x - min_x) as u32, (max_y - min_y) as u32)?;
    for pixel in canvas.pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[0, 0, 0, 255]);
    }
    for (image, x, y, width, height) in entries {
        canvas.blit_nearest(image, x - min_x, y - min_y, width, height);
    }
    canvas.scale = max_scale;
    canvas.region.x = min_x;
    canvas.region.y = min_y;
    Ok(canvas)
}

fn format_scale(scale: f64) -> String {
    if scale.fract().abs() < f64::EPSILON {
        format!("{scale:.0}")
    } else {
        format!("{scale:.2}")
    }
}

fn transform_name(transform: i32) -> &'static str {
    match transform {
        0 => "normal",
        1 => "90",
        2 => "180",
        3 => "270",
        4 => "flipped",
        5 => "flipped-90",
        6 => "flipped-180",
        7 => "flipped-270",
        _ => "unknown",
    }
}

fn select_full_output<'a>(
    outputs: &'a [OutputInfo],
    focused_name: Option<&str>,
) -> Option<&'a OutputInfo> {
    focused_name
        .and_then(|name| outputs.iter().find(|output| output.name == name))
        .or_else(|| outputs.first())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imagebuf::Region;

    fn output(name: &str, x: i32, scale: f64) -> OutputInfo {
        OutputInfo {
            id: 0,
            name: name.into(),
            physical_width: 100,
            physical_height: 100,
            x,
            y: 0,
            logical_width: 100,
            logical_height: 100,
            scale,
            transform: 0,
        }
    }

    #[test]
    fn composition_handles_negative_positions_and_scale() {
        let mut left = ImageBuffer::new(100, 100).unwrap();
        left.region = Region::default();
        let right = ImageBuffer::new(200, 200).unwrap();
        let result = compose_outputs(vec![
            (left, output("a", -100, 1.0)),
            (right, output("b", 0, 2.0)),
        ])
        .unwrap();
        assert_eq!((result.width, result.height), (400, 200));
        assert_eq!(result.scale, 2.0);
    }

    #[test]
    fn full_output_prefers_backend_focus_and_falls_back_to_first() {
        let outputs = vec![output("DP-1", 0, 1.0), output("DP-2", 100, 1.0)];
        assert_eq!(
            select_full_output(&outputs, Some("DP-2")).unwrap().name,
            "DP-2"
        );
        assert_eq!(
            select_full_output(&outputs, Some("missing")).unwrap().name,
            "DP-1"
        );
        assert_eq!(select_full_output(&outputs, None).unwrap().name, "DP-1");
        assert!(select_full_output(&[], None).is_none());
    }
}
