use smithay_client_toolkit::{
    delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shm::{Shm, ShmHandler, raw::RawPool},
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop,
    globals::registry_queue_init,
    protocol::{wl_buffer, wl_output, wl_shm},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

use crate::{
    backend::OutputInfo,
    error::{Error, Result},
    imagebuf::{ImageBuffer, Region},
};

struct Allocation {
    pool: RawPool,
    buffer: wl_buffer::WlBuffer,
    width: u32,
    height: u32,
    stride: u32,
    format: u32,
}

struct CaptureState {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    allocation: Option<Allocation>,
    ready: bool,
    failed: bool,
    y_inverted: bool,
    protocol_error: Option<String>,
}

pub struct WaylandCapture {
    queue: EventQueue<CaptureState>,
    state: CaptureState,
    manager: Option<ZwlrScreencopyManagerV1>,
}

impl WaylandCapture {
    pub fn connect() -> Result<Self> {
        let connection = Connection::connect_to_env()
            .map_err(|err| Error::Wayland(format!("connect: {err}")))?;
        let (globals, mut queue) = registry_queue_init::<CaptureState>(&connection)
            .map_err(|err| Error::Wayland(format!("registry: {err}")))?;
        let qh = queue.handle();
        let registry_state = RegistryState::new(&globals);
        let output_state = OutputState::new(&globals, &qh);
        let shm = Shm::bind(&globals, &qh).map_err(|_| Error::MissingProtocol("wl_shm"))?;
        let manager = globals
            .bind::<ZwlrScreencopyManagerV1, _, _>(&qh, 1..=3, ())
            .ok();
        let mut state = CaptureState {
            registry_state,
            output_state,
            shm,
            allocation: None,
            ready: false,
            failed: false,
            y_inverted: false,
            protocol_error: None,
        };
        queue
            .roundtrip(&mut state)
            .map_err(|err| Error::Wayland(format!("output roundtrip: {err}")))?;
        Ok(Self {
            queue,
            state,
            manager,
        })
    }

    pub fn outputs(&self) -> Vec<OutputInfo> {
        let mut outputs: Vec<_> = self
            .state
            .output_state
            .outputs()
            .filter_map(|output| {
                let info = self.state.output_state.info(&output)?;
                Some(output_info(info))
            })
            .collect();
        outputs.sort_by(|a, b| a.name.cmp(&b.name));
        outputs
    }

    pub fn supports_screencopy(&self) -> bool {
        self.manager.is_some()
    }

    pub fn capture_output(
        &mut self,
        info: &OutputInfo,
        overlay_cursor: bool,
    ) -> Result<ImageBuffer> {
        let output = self.find_output(info)?;
        let qh = self.queue.handle();
        self.reset_frame();
        let frame = self
            .manager
            .as_ref()
            .ok_or(Error::MissingProtocol("wlr-screencopy-unstable-v1"))?
            .capture_output(i32::from(overlay_cursor), &output, &qh, ());
        let mut image = self.finish_frame(&frame)?;
        if info.transform != 0 {
            image = image.apply_inverse_transform(info.transform)?;
        }
        image.scale = valid_scale(info.scale);
        image.region = Region {
            x: info.x,
            y: info.y,
            width: image.width as i32,
            height: image.height as i32,
            output: info.name.clone(),
        };
        Ok(image)
    }

    pub fn capture_region(
        &mut self,
        info: &OutputInfo,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        overlay_cursor: bool,
    ) -> Result<ImageBuffer> {
        if width <= 0 || height <= 0 {
            return Err(Error::InvalidBuffer("empty capture region".into()));
        }
        let output = self.find_output(info)?;
        let qh = self.queue.handle();
        self.reset_frame();
        let frame = self
            .manager
            .as_ref()
            .ok_or(Error::MissingProtocol("wlr-screencopy-unstable-v1"))?
            .capture_output_region(
                i32::from(overlay_cursor),
                &output,
                x,
                y,
                width,
                height,
                &qh,
                (),
            );
        let mut image = self.finish_frame(&frame)?;
        image.scale = valid_scale(info.scale);
        image.region = Region {
            x: info.x + x,
            y: info.y + y,
            width,
            height,
            output: info.name.clone(),
        };
        Ok(image)
    }

    fn find_output(&self, requested: &OutputInfo) -> Result<wl_output::WlOutput> {
        self.state
            .output_state
            .outputs()
            .find(|output| {
                self.state
                    .output_state
                    .info(output)
                    .is_some_and(|info| info.id == requested.id)
            })
            .ok_or_else(|| Error::OutputNotFound(requested.name.clone()))
    }

    fn reset_frame(&mut self) {
        self.state.allocation = None;
        self.state.ready = false;
        self.state.failed = false;
        self.state.y_inverted = false;
        self.state.protocol_error = None;
    }

    fn finish_frame(&mut self, frame: &ZwlrScreencopyFrameV1) -> Result<ImageBuffer> {
        while !self.state.ready && !self.state.failed && self.state.protocol_error.is_none() {
            self.queue
                .blocking_dispatch(&mut self.state)
                .map_err(|err| Error::Wayland(format!("capture dispatch: {err}")))?;
        }
        frame.destroy();
        if let Some(message) = self.state.protocol_error.take() {
            return Err(Error::InvalidBuffer(message));
        }
        if self.state.failed {
            return Err(Error::CaptureFailed);
        }
        let y_inverted = self.state.y_inverted;
        let mut allocation = self
            .state
            .allocation
            .take()
            .ok_or_else(|| Error::InvalidBuffer("compositor returned no buffer".into()))?;
        ImageBuffer::from_wayland(
            allocation.width,
            allocation.height,
            allocation.stride,
            allocation.format,
            allocation.pool.mmap(),
            y_inverted,
        )
    }
}

fn output_info(info: smithay_client_toolkit::output::OutputInfo) -> OutputInfo {
    let transform = u32::from(info.transform) as i32;
    let mode = info
        .modes
        .iter()
        .find(|mode| mode.current)
        .or_else(|| info.modes.iter().find(|mode| mode.preferred))
        .or_else(|| info.modes.first());
    let integer_scale = info.scale_factor.max(1) as f64;
    let (mut physical_width, mut physical_height) = mode.map_or((0, 0), |mode| {
        (
            mode.dimensions.0.max(0) as u32,
            mode.dimensions.1.max(0) as u32,
        )
    });
    let (mut logical_width, mut logical_height) = info
        .logical_size
        .map_or((0, 0), |size| (size.0.max(0) as u32, size.1.max(0) as u32));

    let swaps_axes = matches!(transform, 1 | 3 | 5 | 7);
    let (oriented_width, oriented_height) = if swaps_axes {
        (physical_height, physical_width)
    } else {
        (physical_width, physical_height)
    };
    if logical_width == 0 || logical_height == 0 {
        logical_width = (oriented_width as f64 / integer_scale).round() as u32;
        logical_height = (oriented_height as f64 / integer_scale).round() as u32;
    }
    if physical_width == 0 || physical_height == 0 {
        let fallback_width = (logical_width as f64 * integer_scale).round() as u32;
        let fallback_height = (logical_height as f64 * integer_scale).round() as u32;
        (physical_width, physical_height) = if swaps_axes {
            (fallback_height, fallback_width)
        } else {
            (fallback_width, fallback_height)
        };
    }

    let scale = fractional_scale(
        physical_width,
        physical_height,
        logical_width,
        logical_height,
        transform,
        integer_scale,
    );
    let (x, y) = info.logical_position.unwrap_or(info.location);
    OutputInfo {
        id: info.id,
        name: info.name.unwrap_or_else(|| format!("output-{}", info.id)),
        physical_width,
        physical_height,
        x,
        y,
        logical_width: logical_width.max(1),
        logical_height: logical_height.max(1),
        scale,
        transform,
    }
}

fn fractional_scale(
    physical_width: u32,
    physical_height: u32,
    logical_width: u32,
    logical_height: u32,
    transform: i32,
    fallback: f64,
) -> f64 {
    if physical_width == 0 || physical_height == 0 || logical_width == 0 || logical_height == 0 {
        return valid_scale(fallback);
    }
    let (width, height) = if matches!(transform, 1 | 3 | 5 | 7) {
        (physical_height, physical_width)
    } else {
        (physical_width, physical_height)
    };
    let x = width as f64 / logical_width as f64;
    let y = height as f64 / logical_height as f64;
    if x.is_finite() && y.is_finite() && (x - y).abs() <= 0.02 {
        valid_scale((x + y) / 2.0)
    } else {
        valid_scale(fallback)
    }
}

fn valid_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for CaptureState {
    fn event(
        state: &mut Self,
        frame: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use zwlr_screencopy_frame_v1::Event;
        match event {
            Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                let raw_format = enum_value(format);
                let bpp = if matches!(raw_format, 0x3432_4752 | 0x3432_4742) {
                    3
                } else {
                    4
                };
                if stride < width.saturating_mul(bpp) {
                    state.protocol_error = Some(format!(
                        "stride {stride} is smaller than width {width} x {bpp}"
                    ));
                    return;
                }
                let Some(size) = (stride as usize).checked_mul(height as usize) else {
                    state.protocol_error = Some("capture buffer is too large".into());
                    return;
                };
                let Ok(mut pool) = RawPool::new(size, &state.shm) else {
                    state.protocol_error = Some("unable to allocate shared memory".into());
                    return;
                };
                let buffer = pool.create_buffer(
                    0,
                    width as i32,
                    height as i32,
                    stride as i32,
                    wl_format(format),
                    (),
                    qh,
                );
                state.allocation = Some(Allocation {
                    pool,
                    buffer,
                    width,
                    height,
                    stride,
                    format: raw_format,
                });
                if frame.version() < 3
                    && let Some(allocation) = &state.allocation
                {
                    frame.copy(&allocation.buffer);
                }
            }
            Event::BufferDone => {
                if let Some(allocation) = &state.allocation {
                    frame.copy(&allocation.buffer);
                } else {
                    state.protocol_error =
                        Some("buffer_done arrived without buffer metadata".into());
                }
            }
            Event::Flags { flags } => {
                state.y_inverted = enum_value(flags) & 1 != 0;
            }
            Event::Ready { .. } => state.ready = true,
            Event::Failed => state.failed = true,
            _ => {}
        }
    }
}

fn enum_value<T>(value: WEnum<T>) -> u32
where
    T: Into<u32>,
{
    match value {
        WEnum::Value(value) => value.into(),
        WEnum::Unknown(value) => value,
    }
}

fn wl_format(value: WEnum<wl_shm::Format>) -> wl_shm::Format {
    match value {
        WEnum::Value(value) => value,
        WEnum::Unknown(0) => wl_shm::Format::Argb8888,
        WEnum::Unknown(1) => wl_shm::Format::Xrgb8888,
        WEnum::Unknown(0x3432_4241) => wl_shm::Format::Abgr8888,
        WEnum::Unknown(0x3432_4258) => wl_shm::Format::Xbgr8888,
        WEnum::Unknown(0x3432_4752) => wl_shm::Format::Rgb888,
        WEnum::Unknown(0x3432_4742) => wl_shm::Format::Bgr888,
        WEnum::Unknown(_) => wl_shm::Format::Argb8888,
    }
}

impl OutputHandler for CaptureState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for CaptureState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for CaptureState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState];
}

delegate_registry!(CaptureState);
smithay_client_toolkit::delegate_dispatch2!(CaptureState);
delegate_noop!(CaptureState: ignore ZwlrScreencopyManagerV1);
delegate_noop!(CaptureState: ignore wl_buffer::WlBuffer);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_fractional_scale_from_logical_size() {
        assert!((fractional_scale(2560, 1440, 2048, 1152, 0, 1.0) - 1.25).abs() < 0.001);
    }

    #[test]
    fn derives_fractional_scale_for_rotated_output() {
        assert!((fractional_scale(2560, 1440, 1152, 2048, 1, 1.0) - 1.25).abs() < 0.001);
    }

    #[test]
    fn inconsistent_dimensions_use_protocol_scale() {
        assert_eq!(fractional_scale(1920, 1080, 1000, 1000, 0, 2.0), 2.0);
    }
}
