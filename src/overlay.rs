use std::{
    collections::VecDeque,
    fs,
    time::{Duration, Instant},
};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::{calloop::EventLoop, calloop_wayland_source::WaylandSource},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{
            CursorIcon, PointerEvent, PointerEventKind, PointerHandler, ThemeSpec, ThemedPointer,
        },
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};
use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::{
    zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1,
    zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1,
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};

use crate::{
    error::{Error, Result},
    imagebuf::{ImageBuffer, Region},
    niri::OutputInfo,
    stitch::Stitcher,
    wayland::WaylandCapture,
};

const LEFT_BUTTON: u32 = 0x110;
const SCROLL_SEAM_TICKS: u8 = 4;

pub struct Background {
    pub info: OutputInfo,
    pub with_cursor: ImageBuffer,
    pub without_cursor: ImageBuffer,
}

pub struct SelectorOptions {
    pub no_confirm: bool,
    pub scroll: bool,
    pub interval: Duration,
    pub initial: Region,
    pub show_cursor: bool,
}

struct Surface {
    name: String,
    layer: LayerSurface,
    viewport: WpViewport,
    logical_width: u32,
    logical_height: u32,
    configured: bool,
    frame_pending: bool,
    redraw_pending: bool,
    with_cursor: ImageBuffer,
    without_cursor: ImageBuffer,
    buffers: VecDeque<Buffer>,
}

#[derive(Default)]
struct Selection {
    surface: Option<usize>,
    anchor: (f64, f64),
    current: (f64, f64),
    dragging: bool,
    present: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Select,
    Scroll,
}

struct ScrollState {
    region: Region,
    local_region: Region,
    output: OutputInfo,
    interval: Duration,
    next_tick: Instant,
    stitcher: Option<Stitcher>,
    kept: usize,
    unmatched_ticks: u8,
    failures: u8,
}

struct SelectorState {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    compositor: CompositorState,
    shm: Shm,
    pool: SlotPool,
    surfaces: Vec<Surface>,
    outputs: Vec<OutputInfo>,
    pointer: Option<ThemedPointer<()>>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    shortcut_seat: Option<wl_seat::WlSeat>,
    shortcuts_manager: ZwpKeyboardShortcutsInhibitManagerV1,
    shortcuts_inhibitors: Vec<ZwpKeyboardShortcutsInhibitorV1>,
    _viewporter: WpViewporter,
    active_surface: Option<usize>,
    pointer_position: (f64, f64),
    selection: Selection,
    shift: bool,
    show_cursor: bool,
    no_confirm: bool,
    phase: Phase,
    scroll: Option<ScrollState>,
    scroll_requested: bool,
    interval: Duration,
    running: bool,
    cancelled: bool,
    result: Option<ImageBuffer>,
    error: Option<Error>,
    theme: Theme,
}

pub fn select_region(
    backgrounds: Vec<Background>,
    options: SelectorOptions,
    capture: &mut WaylandCapture,
) -> Result<Option<ImageBuffer>> {
    let connection = Connection::connect_to_env()
        .map_err(|err| Error::Wayland(format!("selector connect: {err}")))?;
    let (globals, mut event_queue) = registry_queue_init::<SelectorState>(&connection)
        .map_err(|err| Error::Wayland(format!("selector registry: {err}")))?;
    let qh = event_queue.handle();
    let compositor = CompositorState::bind(&globals, &qh)
        .map_err(|_| Error::MissingProtocol("wl_compositor"))?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .map_err(|_| Error::MissingProtocol("wlr-layer-shell-unstable-v1"))?;
    let shm = Shm::bind(&globals, &qh).map_err(|_| Error::MissingProtocol("wl_shm"))?;
    let shortcuts_manager = globals
        .bind::<ZwpKeyboardShortcutsInhibitManagerV1, _, _>(&qh, 1..=1, ())
        .map_err(|_| Error::MissingProtocol("keyboard-shortcuts-inhibit-unstable-v1"))?;
    let viewporter = globals
        .bind::<WpViewporter, _, _>(&qh, 1..=1, ())
        .map_err(|_| Error::MissingProtocol("wp_viewporter"))?;
    let mut state = SelectorState {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        compositor,
        pool: SlotPool::new(4, &shm)
            .map_err(|err| Error::Wayland(format!("selector shared memory: {err}")))?,
        shm,
        surfaces: Vec::new(),
        outputs: backgrounds.iter().map(|item| item.info.clone()).collect(),
        pointer: None,
        keyboard: None,
        shortcut_seat: None,
        shortcuts_manager,
        shortcuts_inhibitors: Vec::new(),
        _viewporter: viewporter.clone(),
        active_surface: None,
        pointer_position: (0.0, 0.0),
        selection: Selection::default(),
        shift: false,
        show_cursor: options.show_cursor,
        no_confirm: options.no_confirm,
        phase: Phase::Select,
        scroll: None,
        scroll_requested: options.scroll,
        interval: options.interval,
        running: true,
        cancelled: false,
        result: None,
        error: None,
        theme: Theme::load(),
    };
    event_queue
        .roundtrip(&mut state)
        .map_err(|err| Error::Wayland(format!("selector output roundtrip: {err}")))?;

    for background in backgrounds {
        let output = state
            .output_state
            .outputs()
            .find(|output| {
                state
                    .output_state
                    .info(output)
                    .and_then(|info| info.name)
                    .as_deref()
                    == Some(background.info.name.as_str())
            })
            .ok_or_else(|| Error::OutputNotFound(background.info.name.clone()))?;
        let wl_surface = state.compositor.create_surface(&qh);
        let viewport = viewporter.get_viewport(&wl_surface, &qh, ());
        let layer = layer_shell.create_layer_surface(
            &qh,
            wl_surface,
            Layer::Overlay,
            Some("dms-screenshot"),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.commit();
        state.surfaces.push(Surface {
            name: background.info.name,
            layer,
            viewport,
            logical_width: background.info.logical_width,
            logical_height: background.info.logical_height,
            configured: false,
            frame_pending: false,
            redraw_pending: true,
            with_cursor: background.with_cursor,
            without_cursor: background.without_cursor,
            buffers: VecDeque::new(),
        });
    }
    state.install_shortcut_inhibitors(&qh);
    state.apply_initial(options.initial);

    let mut event_loop: EventLoop<SelectorState> =
        EventLoop::try_new().map_err(|err| Error::Wayland(format!("event loop: {err}")))?;
    WaylandSource::new(connection, event_queue)
        .insert(event_loop.handle())
        .map_err(|err| Error::Wayland(format!("event source: {err}")))?;

    while state.running {
        event_loop
            .dispatch(Duration::from_millis(10), &mut state)
            .map_err(|err| Error::Wayland(format!("selector dispatch: {err}")))?;
        if state.scroll_due() {
            state.capture_scroll_frame(capture, &qh);
        }
    }
    if let Some(error) = state.error {
        return Err(error);
    }
    if state.cancelled {
        return Ok(None);
    }
    Ok(state.result)
}

impl SelectorState {
    fn install_shortcut_inhibitors(&mut self, qh: &QueueHandle<Self>) {
        if !self.shortcuts_inhibitors.is_empty() {
            return;
        }
        let Some(seat) = self.shortcut_seat.as_ref() else {
            return;
        };
        self.shortcuts_inhibitors = self
            .surfaces
            .iter()
            .map(|surface| {
                self.shortcuts_manager
                    .inhibit_shortcuts(surface.layer.wl_surface(), seat, qh, ())
            })
            .collect();
    }

    fn apply_initial(&mut self, initial: Region) {
        if initial.is_empty() {
            return;
        }
        let Some(index) = self
            .surfaces
            .iter()
            .position(|surface| surface.name == initial.output)
        else {
            return;
        };
        let info = &self.outputs[index];
        self.selection = Selection {
            surface: Some(index),
            anchor: ((initial.x - info.x) as f64, (initial.y - info.y) as f64),
            current: (
                (initial.x - info.x + initial.width - 1) as f64,
                (initial.y - info.y + initial.height - 1) as f64,
            ),
            dragging: false,
            present: true,
        };
    }

    fn redraw_all(&mut self, qh: &QueueHandle<Self>) {
        for index in 0..self.surfaces.len() {
            self.redraw(index, qh);
        }
    }

    fn redraw(&mut self, index: usize, qh: &QueueHandle<Self>) {
        if !self.surfaces[index].configured {
            self.surfaces[index].redraw_pending = true;
            return;
        }
        // Pointer motion can arrive much faster than a full-screen software render.  Submitting
        // one buffer for every event starves the event queue and can make the compositor close
        // the client.  Keep only the newest state and render it on the next compositor frame.
        if self.surfaces[index].frame_pending {
            self.surfaces[index].redraw_pending = true;
            return;
        }
        let width = self.surfaces[index].logical_width.max(1);
        let height = self.surfaces[index].logical_height.max(1);
        let rgba = self.render(index, width, height);
        let Ok((buffer, canvas)) = self.pool.create_buffer(
            width as i32,
            height as i32,
            width as i32 * 4,
            wl_shm::Format::Argb8888,
        ) else {
            self.error = Some(Error::Wayland("unable to allocate overlay buffer".into()));
            self.running = false;
            return;
        };
        for (source, target) in rgba.chunks_exact(4).zip(canvas.chunks_exact_mut(4)) {
            let alpha = source[3] as u16;
            target.copy_from_slice(&[
                (source[2] as u16 * alpha / 255) as u8,
                (source[1] as u16 * alpha / 255) as u8,
                (source[0] as u16 * alpha / 255) as u8,
                source[3],
            ]);
        }
        let surface = &mut self.surfaces[index];
        surface
            .layer
            .wl_surface()
            .damage_buffer(0, 0, width as i32, height as i32);
        surface
            .viewport
            .set_destination(width as i32, height as i32);
        if let Err(error) = buffer.attach_to(surface.layer.wl_surface()) {
            self.error = Some(Error::Wayland(format!(
                "unable to attach overlay buffer: {error}"
            )));
            self.running = false;
            return;
        }
        surface
            .layer
            .wl_surface()
            .frame(qh, FrameCallbackData(surface.layer.wl_surface().clone()));
        surface.layer.commit();
        surface.frame_pending = true;
        surface.redraw_pending = false;
        surface.buffers.push_back(buffer);
        while surface.buffers.len() > 3 {
            surface.buffers.pop_front();
        }
    }

    fn render(&self, index: usize, width: u32, height: u32) -> Vec<u8> {
        let surface = &self.surfaces[index];
        let source = if self.show_cursor {
            &surface.with_cursor
        } else {
            &surface.without_cursor
        };
        let mut rgba = vec![0; width as usize * height as usize * 4];
        if self.phase == Phase::Select {
            for y in 0..height {
                let sy = y as usize * source.height as usize / height as usize;
                for x in 0..width {
                    let sx = x as usize * source.width as usize / width as usize;
                    let si = (sy * source.width as usize + sx) * 4;
                    let di = (y as usize * width as usize + x as usize) * 4;
                    // Match DMS' Material scrim (black at alpha 118).
                    rgba[di] = (source.pixels[si] as u16 * 137 / 255) as u8;
                    rgba[di + 1] = (source.pixels[si + 1] as u16 * 137 / 255) as u8;
                    rgba[di + 2] = (source.pixels[si + 2] as u16 * 137 / 255) as u8;
                    rgba[di + 3] = 255;
                }
            }
            if self.selection.present && self.selection.surface == Some(index) {
                let rect = selection_rect(&self.selection, width, height);
                for y in rect.1..rect.1 + rect.3 {
                    let sy = y as usize * source.height as usize / height as usize;
                    for x in rect.0..rect.0 + rect.2 {
                        let sx = x as usize * source.width as usize / width as usize;
                        let si = (sy * source.width as usize + sx) * 4;
                        let di = (y as usize * width as usize + x as usize) * 4;
                        rgba[di..di + 4].copy_from_slice(&source.pixels[si..si + 4]);
                    }
                }
                draw_selection_border(&mut rgba, width, height, rect, self.theme);
                draw_label(
                    &mut rgba,
                    width,
                    height,
                    rect,
                    &format!("{}x{}", rect.2, rect.3),
                    self.theme,
                );
            }
            if !self.selection.dragging {
                draw_hud(
                    &mut rgba,
                    width,
                    height,
                    self.show_cursor,
                    self.no_confirm,
                    self.theme,
                );
            }
        } else {
            for pixel in rgba.chunks_exact_mut(4) {
                pixel.copy_from_slice(&[0, 0, 0, 102]);
            }
            if let Some(scroll) = &self.scroll
                && self.selection.surface == Some(index)
            {
                let rect = (
                    scroll.local_region.x.max(0) as u32,
                    scroll.local_region.y.max(0) as u32,
                    scroll.local_region.width.max(1) as u32,
                    scroll.local_region.height.max(1) as u32,
                );
                clear_rect(&mut rgba, width, height, rect);
                draw_outside_border(&mut rgba, width, height, rect, self.theme.accent);
                draw_scroll_bar(
                    &mut rgba,
                    width,
                    height,
                    scroll.kept,
                    scroll.stitcher.as_ref().map_or(0, Stitcher::rows),
                    self.theme,
                );
            }
        }
        rgba
    }

    fn selected_rect(&self) -> Option<(usize, Region, Region)> {
        let index = self.selection.surface?;
        let surface = &self.surfaces[index];
        let (x, y, width, height) = selection_rect(
            &self.selection,
            surface.logical_width,
            surface.logical_height,
        );
        let source = if self.show_cursor {
            &surface.with_cursor
        } else {
            &surface.without_cursor
        };
        let sx = source.width as f64 / surface.logical_width.max(1) as f64;
        let sy = source.height as f64 / surface.logical_height.max(1) as f64;
        let physical = Region {
            x: (x as f64 * sx).round() as i32,
            y: (y as f64 * sy).round() as i32,
            width: (width as f64 * sx).round().max(1.0) as i32,
            height: (height as f64 * sy).round().max(1.0) as i32,
            output: surface.name.clone(),
        };
        let info = &self.outputs[index];
        let logical = Region {
            x: x as i32,
            y: y as i32,
            width: width as i32,
            height: height as i32,
            output: surface.name.clone(),
        };
        let global = Region {
            x: info.x + logical.x,
            y: info.y + logical.y,
            ..logical.clone()
        };
        Some((index, physical, global))
    }

    fn confirm_selection(&mut self, qh: &QueueHandle<Self>) {
        let Some((index, physical, global)) = self.selected_rect() else {
            return;
        };
        if !self.scroll_requested {
            let source = if self.show_cursor {
                &self.surfaces[index].with_cursor
            } else {
                &self.surfaces[index].without_cursor
            };
            match source.crop(physical.x, physical.y, physical.width, physical.height) {
                Ok(mut image) => {
                    image.region = global;
                    image.scale = self.outputs[index].scale;
                    self.result = Some(image);
                }
                Err(error) => self.error = Some(error),
            }
            self.running = false;
            return;
        }
        if self.outputs[index].transform != 0 {
            self.error = Some(Error::RotatedScroll);
            self.running = false;
            return;
        }
        let local_region = Region {
            x: global.x - self.outputs[index].x,
            y: global.y - self.outputs[index].y,
            ..global.clone()
        };
        self.scroll = Some(ScrollState {
            region: global,
            local_region,
            output: self.outputs[index].clone(),
            interval: self.interval,
            next_tick: Instant::now(),
            stitcher: None,
            kept: 0,
            unmatched_ticks: 0,
            failures: 0,
        });
        self.phase = Phase::Scroll;
        self.set_scroll_input_region(index, qh);
        self.redraw_all(qh);
    }

    fn set_scroll_input_region(&self, selected: usize, qh: &QueueHandle<Self>) {
        for (index, surface) in self.surfaces.iter().enumerate() {
            let region = self.compositor.wl_compositor().create_region(qh, ());
            if index == selected {
                let (x, y, width, height) =
                    scroll_bar_geometry(surface.logical_width, surface.logical_height);
                region.add(x as i32, y as i32, width as i32, height as i32);
            }
            surface.layer.wl_surface().set_input_region(Some(&region));
            region.destroy();
            surface.layer.commit();
        }
    }

    fn scroll_due(&self) -> bool {
        self.phase == Phase::Scroll
            && self.scroll.as_ref().is_some_and(|scroll| {
                Instant::now() >= scroll.next_tick
                    && !scroll.stitcher.as_ref().is_some_and(Stitcher::is_full)
            })
    }

    fn capture_scroll_frame(&mut self, capture: &mut WaylandCapture, qh: &QueueHandle<Self>) {
        let Some(scroll) = &mut self.scroll else {
            return;
        };
        let result = capture.capture_region(
            &scroll.output,
            scroll.local_region.x,
            scroll.local_region.y,
            scroll.local_region.width,
            scroll.local_region.height,
            false,
        );
        scroll.next_tick = Instant::now() + scroll.interval;
        match result.and_then(|image| {
            let stitcher = scroll
                .stitcher
                .get_or_insert_with(|| Stitcher::new(image.width));
            stitcher.push(&image).map(|result| (image, result))
        }) {
            Ok((image, result)) => {
                scroll.failures = 0;
                if result.placed {
                    scroll.unmatched_ticks = 0;
                    if result.added_rows > 0 {
                        scroll.kept += 1;
                    }
                } else {
                    scroll.unmatched_ticks += 1;
                    if scroll.unmatched_ticks >= SCROLL_SEAM_TICKS {
                        if let Ok(added) = scroll.stitcher.as_mut().unwrap().seam_append(&image)
                            && added > 0
                        {
                            scroll.kept += 1;
                        }
                        scroll.unmatched_ticks = 0;
                    }
                }
                if (!result.placed || result.added_rows > 0)
                    && let Some(index) = self.selection.surface
                {
                    self.redraw(index, qh);
                }
            }
            Err(error) => {
                scroll.failures += 1;
                if scroll.failures >= 5 {
                    self.error = Some(error);
                    self.running = false;
                }
            }
        }
    }

    fn finish_scroll(&mut self) {
        let Some(scroll) = self.scroll.take() else {
            return;
        };
        let Some(stitcher) = scroll.stitcher else {
            self.cancelled = true;
            self.running = false;
            return;
        };
        if stitcher.rows() == 0 {
            self.cancelled = true;
            self.running = false;
            return;
        }
        match stitcher.finish() {
            Ok(mut image) => {
                image.scale = scroll.output.scale;
                image.region = scroll.region;
                self.result = Some(image);
            }
            Err(error) => self.error = Some(error),
        }
        self.running = false;
    }
}

impl CompositorHandler for SelectorState {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        let Some(index) = self
            .surfaces
            .iter()
            .position(|item| item.layer.wl_surface() == surface)
        else {
            return;
        };
        self.surfaces[index].frame_pending = false;
        if self.surfaces[index].redraw_pending {
            self.redraw(index, qh);
        }
    }
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for SelectorState {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.cancelled = true;
        self.running = false;
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        if let Some(index) = self
            .surfaces
            .iter()
            .position(|surface| surface.layer.wl_surface() == layer.wl_surface())
        {
            if configure.new_size.0 > 0 {
                self.surfaces[index].logical_width = configure.new_size.0;
            }
            if configure.new_size.1 > 0 {
                self.surfaces[index].logical_height = configure.new_size.1;
            }
            self.surfaces[index].configured = true;
            self.redraw(index, qh);
        }
    }
}

impl SeatHandler for SelectorState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            let surface = self.compositor.create_surface(qh);
            self.pointer = self
                .seat_state
                .get_pointer_with_theme::<SelectorState, ()>(
                    qh,
                    &seat,
                    self.shm.wl_shm(),
                    surface,
                    ThemeSpec::System,
                )
                .ok();
        }
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.shortcut_seat = Some(seat.clone());
            self.keyboard = self.seat_state.get_keyboard(qh, &seat, None).ok();
            self.install_shortcut_inhibitors(qh);
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            self.pointer.take();
        }
        if capability == Capability::Keyboard {
            self.keyboard.take();
            self.shortcut_seat.take();
            for inhibitor in self.shortcuts_inhibitors.drain(..) {
                inhibitor.destroy();
            }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for SelectorState {
    fn pointer_frame(
        &mut self,
        connection: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let Some(index) = self
                .surfaces
                .iter()
                .position(|surface| surface.layer.wl_surface() == &event.surface)
            else {
                continue;
            };
            self.active_surface = Some(index);
            self.pointer_position = event.position;
            match event.kind {
                PointerEventKind::Enter { .. } => {
                    if let Some(pointer) = &self.pointer {
                        let _ = pointer.set_cursor(connection, CursorIcon::Crosshair);
                    }
                }
                PointerEventKind::Motion { .. } => {
                    if self.phase == Phase::Select && self.selection.dragging {
                        let mut current = event.position;
                        if self.shift {
                            let dx = current.0 - self.selection.anchor.0;
                            let dy = current.1 - self.selection.anchor.1;
                            let size = dx.abs().max(dy.abs());
                            current.0 = self.selection.anchor.0 + size.copysign(dx);
                            current.1 = self.selection.anchor.1 + size.copysign(dy);
                        }
                        self.selection.current = current;
                        self.redraw_all(qh);
                    }
                }
                PointerEventKind::Press { button, .. } if button == LEFT_BUTTON => {
                    if self.phase == Phase::Scroll {
                        if scroll_bar_hit(
                            self.surfaces[index].logical_width,
                            self.surfaces[index].logical_height,
                            event.position,
                        ) == 1
                        {
                            self.finish_scroll();
                        } else if scroll_bar_hit(
                            self.surfaces[index].logical_width,
                            self.surfaces[index].logical_height,
                            event.position,
                        ) == 2
                        {
                            self.cancelled = true;
                            self.running = false;
                        }
                    } else {
                        self.selection = Selection {
                            surface: Some(index),
                            anchor: event.position,
                            current: event.position,
                            dragging: true,
                            present: true,
                        };
                        self.redraw_all(qh);
                    }
                }
                PointerEventKind::Release { button, .. }
                    if button == LEFT_BUTTON && self.phase == Phase::Select =>
                {
                    self.selection.dragging = false;
                    self.redraw_all(qh);
                    if self.no_confirm {
                        self.confirm_selection(qh);
                    }
                }
                PointerEventKind::Press { .. } if self.phase == Phase::Select => {
                    self.cancelled = true;
                    self.running = false;
                }
                _ => {}
            }
        }
    }
}

impl KeyboardHandler for SelectorState {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }
    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
    }
    fn press_key(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        match event.keysym {
            Keysym::Escape => {
                self.cancelled = true;
                self.running = false;
            }
            Keysym::Return | Keysym::KP_Enter | Keysym::space => {
                if self.phase == Phase::Scroll {
                    self.finish_scroll();
                } else if self.selection.present {
                    self.confirm_selection(qh);
                }
            }
            Keysym::p | Keysym::P if self.phase == Phase::Select => {
                self.show_cursor = !self.show_cursor;
                self.redraw_all(qh);
            }
            _ => {}
        }
    }
    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }
    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }
    fn update_modifiers(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        if self.shift != modifiers.shift {
            self.shift = modifiers.shift;
            self.redraw_all(qh);
        }
    }
}

impl OutputHandler for SelectorState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for SelectorState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}
impl ProvidesRegistryState for SelectorState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_registry!(SelectorState);
smithay_client_toolkit::delegate_dispatch2!(SelectorState);
wayland_client::delegate_noop!(SelectorState: ignore wayland_client::protocol::wl_region::WlRegion);
wayland_client::delegate_noop!(SelectorState: ignore ZwpKeyboardShortcutsInhibitManagerV1);
wayland_client::delegate_noop!(SelectorState: ignore ZwpKeyboardShortcutsInhibitorV1);
wayland_client::delegate_noop!(SelectorState: ignore WpViewporter);
wayland_client::delegate_noop!(SelectorState: ignore WpViewport);

fn selection_rect(selection: &Selection, width: u32, height: u32) -> (u32, u32, u32, u32) {
    let x0 = selection
        .anchor
        .0
        .min(selection.current.0)
        .floor()
        .clamp(0.0, width.saturating_sub(1) as f64) as u32;
    let y0 = selection
        .anchor
        .1
        .min(selection.current.1)
        .floor()
        .clamp(0.0, height.saturating_sub(1) as f64) as u32;
    let x1 = selection
        .anchor
        .0
        .max(selection.current.0)
        .ceil()
        .clamp(0.0, width.saturating_sub(1) as f64) as u32;
    let y1 = selection
        .anchor
        .1
        .max(selection.current.1)
        .ceil()
        .clamp(0.0, height.saturating_sub(1) as f64) as u32;
    (x0, y0, x1 - x0 + 1, y1 - y0 + 1)
}

#[derive(Clone, Copy)]
struct Theme {
    background: [u8; 4],
    text: [u8; 4],
    accent: [u8; 4],
}
impl Default for Theme {
    fn default() -> Self {
        Self {
            background: [30, 30, 30, 245],
            text: [255, 255, 255, 255],
            accent: [100, 180, 255, 255],
        }
    }
}
impl Theme {
    fn load() -> Self {
        let Some(home) = std::env::var_os("HOME") else {
            return Self::default();
        };
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(home).join(".cache"));
        let Ok(value) = fs::read_to_string(cache.join("DankMaterialShell/dms-colors.json")) else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&value) else {
            return Self::default();
        };
        let mode = if is_light_mode() { "light" } else { "dark" };
        let colors = &value["colors"][mode];
        Self {
            background: parse_color(colors["surface"].as_str())
                .map(|rgb| [rgb[0], rgb[1], rgb[2], 245])
                .unwrap_or(Self::default().background),
            text: parse_color(colors["on_surface"].as_str())
                .map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
                .unwrap_or(Self::default().text),
            accent: parse_color(colors["primary"].as_str())
                .map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
                .unwrap_or(Self::default().accent),
        }
    }
}

fn is_light_mode() -> bool {
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .is_some_and(|value| matches!(value.trim(), "'prefer-light'" | "'default'"))
}

fn parse_color(value: Option<&str>) -> Option<[u8; 3]> {
    let value = value?;
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 6 {
        return None;
    }
    Some([
        u8::from_str_radix(&value[0..2], 16).ok()?,
        u8::from_str_radix(&value[2..4], 16).ok()?,
        u8::from_str_radix(&value[4..6], 16).ok()?,
    ])
}

fn clear_rect(data: &mut [u8], width: u32, height: u32, rect: (u32, u32, u32, u32)) {
    for y in rect.1.saturating_sub(2)..(rect.1 + rect.3 + 2).min(height) {
        for x in rect.0.saturating_sub(2)..(rect.0 + rect.2 + 2).min(width) {
            let i = (y as usize * width as usize + x as usize) * 4;
            data[i..i + 4].fill(0);
        }
    }
}

fn draw_border_lines(
    data: &mut [u8],
    width: u32,
    height: u32,
    rect: (u32, u32, u32, u32),
    thickness: u32,
    color: [u8; 4],
) {
    if rect.2 == 0 || rect.3 == 0 || thickness == 0 {
        return;
    }
    let thickness = thickness.min(rect.2).min(rect.3);
    fill_rect(
        data,
        width,
        height,
        (rect.0, rect.1, rect.2, thickness),
        color,
    );
    fill_rect(
        data,
        width,
        height,
        (
            rect.0,
            rect.1.saturating_add(rect.3).saturating_sub(thickness),
            rect.2,
            thickness,
        ),
        color,
    );
    fill_rect(
        data,
        width,
        height,
        (rect.0, rect.1, thickness, rect.3),
        color,
    );
    fill_rect(
        data,
        width,
        height,
        (
            rect.0.saturating_add(rect.2).saturating_sub(thickness),
            rect.1,
            thickness,
            rect.3,
        ),
        color,
    );
}

fn draw_selection_border(
    data: &mut [u8],
    width: u32,
    height: u32,
    rect: (u32, u32, u32, u32),
    theme: Theme,
) {
    let left = u32::from(rect.0 > 0);
    let top = u32::from(rect.1 > 0);
    let right = u32::from(rect.0.saturating_add(rect.2) < width);
    let bottom = u32::from(rect.1.saturating_add(rect.3) < height);
    let keyline = (
        rect.0.saturating_sub(left),
        rect.1.saturating_sub(top),
        rect.2.saturating_add(left).saturating_add(right),
        rect.3.saturating_add(top).saturating_add(bottom),
    );
    draw_border_lines(data, width, height, keyline, 1, [0, 0, 0, 150]);
    draw_border_lines(data, width, height, rect, 2, theme.accent);

    let handle = 16.min(rect.2).min(rect.3);
    if handle == 0 {
        return;
    }
    let thick = 4.min(handle);
    let right_x = rect.0 + rect.2 - handle;
    let right_edge = rect.0 + rect.2 - thick;
    let bottom_y = rect.1 + rect.3 - handle;
    let bottom_edge = rect.1 + rect.3 - thick;
    for corner in [
        (rect.0, rect.1, handle, thick),
        (rect.0, rect.1, thick, handle),
        (right_x, rect.1, handle, thick),
        (right_edge, rect.1, thick, handle),
        (rect.0, bottom_edge, handle, thick),
        (rect.0, bottom_y, thick, handle),
        (right_x, bottom_edge, handle, thick),
        (right_edge, bottom_y, thick, handle),
    ] {
        fill_rect(data, width, height, corner, theme.accent);
    }
}

fn draw_outside_border(
    data: &mut [u8],
    width: u32,
    height: u32,
    rect: (u32, u32, u32, u32),
    color: [u8; 4],
) {
    let x_end = rect.0.saturating_add(rect.2).min(width);
    let y_end = rect.1.saturating_add(rect.3).min(height);
    for offset in 1..=2 {
        if let Some(y) = rect.1.checked_sub(offset) {
            for x in rect.0..x_end {
                put_pixel(data, width, x, y, color);
            }
        }
        if let Some(y) = y_end.checked_add(offset - 1).filter(|y| *y < height) {
            for x in rect.0..x_end {
                put_pixel(data, width, x, y, color);
            }
        }
        if let Some(x) = rect.0.checked_sub(offset) {
            for y in rect.1..y_end {
                put_pixel(data, width, x, y, color);
            }
        }
        if let Some(x) = x_end.checked_add(offset - 1).filter(|x| *x < width) {
            for y in rect.1..y_end {
                put_pixel(data, width, x, y, color);
            }
        }
    }
}

fn draw_label(
    data: &mut [u8],
    width: u32,
    height: u32,
    rect: (u32, u32, u32, u32),
    text: &str,
    theme: Theme,
) {
    let text_width = text.len() as u32 * 6;
    let chip_width = text_width + 16;
    let chip_height = 15;
    let chip_x = (rect.0 + rect.2 / 2)
        .saturating_sub(chip_width / 2)
        .min(width.saturating_sub(chip_width));
    let chip_y = if rect.1 + rect.3 + 10 + chip_height <= height {
        rect.1 + rect.3 + 10
    } else {
        rect.1.saturating_sub(chip_height + 10)
    };
    fill_rounded_rect(
        data,
        width,
        height,
        (chip_x, chip_y + 2, chip_width, chip_height),
        7,
        [0, 0, 0, 90],
    );
    fill_rounded_rect(
        data,
        width,
        height,
        (chip_x, chip_y, chip_width, chip_height),
        7,
        theme.background,
    );
    draw_text(
        data,
        width,
        height,
        chip_x + 8,
        chip_y + 4,
        text,
        theme.text,
    );
}

fn draw_hud(
    data: &mut [u8],
    width: u32,
    height: u32,
    cursor: bool,
    no_confirm: bool,
    theme: Theme,
) {
    let capture_key = if no_confirm { "release" } else { "enter" };
    let cursor_desc = if cursor { "hide cursor" } else { "show cursor" };
    let items = [
        (capture_key, "capture"),
        ("p", cursor_desc),
        ("esc", "cancel"),
    ];
    let content_width = items
        .iter()
        .map(|(key, desc)| (key.len() + 1 + desc.len()) as u32 * 6)
        .sum::<u32>()
        + (items.len() as u32 - 1) * 18;
    let bar_width = (content_width + 24).min(width);
    let x = (width - bar_width) / 2;
    let y = height.saturating_sub(34);
    fill_rounded_rect(
        data,
        width,
        height,
        (x, y, bar_width, 24),
        10,
        theme.background,
    );
    let mut text_x = x + 12;
    for (index, (key, desc)) in items.into_iter().enumerate() {
        draw_text(data, width, height, text_x, y + 8, key, theme.accent);
        text_x += key.len() as u32 * 6;
        draw_text(data, width, height, text_x, y + 8, " ", theme.text);
        text_x += 6;
        draw_text(data, width, height, text_x, y + 8, desc, theme.text);
        text_x += desc.len() as u32 * 6;
        if index + 1 < items.len() {
            text_x += 18;
        }
    }
}

fn scroll_bar_geometry(width: u32, height: u32) -> (u32, u32, u32, u32) {
    let bar_width = 330.min(width);
    (
        (width - bar_width) / 2,
        height.saturating_sub(72),
        bar_width,
        48.min(height),
    )
}

fn draw_scroll_bar(
    data: &mut [u8],
    width: u32,
    height: u32,
    shots: usize,
    rows: usize,
    theme: Theme,
) {
    let bar = scroll_bar_geometry(width, height);
    fill_rect(data, width, height, bar, theme.background);
    fill_rect(
        data,
        width,
        height,
        (bar.0 + 12, bar.1 + 12, 54, 24),
        theme.accent,
    );
    fill_rect(
        data,
        width,
        height,
        (bar.0 + 78, bar.1 + 12, 66, 24),
        [70, 70, 70, 255],
    );
    draw_text(
        data,
        width,
        height,
        bar.0 + 22,
        bar.1 + 20,
        "done",
        [10, 10, 10, 255],
    );
    draw_text(
        data,
        width,
        height,
        bar.0 + 86,
        bar.1 + 20,
        "cancel",
        theme.text,
    );
    draw_text(
        data,
        width,
        height,
        bar.0 + 158,
        bar.1 + 20,
        &format!("{shots} shots {rows}px"),
        theme.text,
    );
}

fn scroll_bar_hit(width: u32, height: u32, position: (f64, f64)) -> u8 {
    let bar = scroll_bar_geometry(width, height);
    let (x, y) = (position.0 as u32, position.1 as u32);
    if y >= bar.1 + 12 && y < bar.1 + 36 {
        if x >= bar.0 + 12 && x < bar.0 + 66 {
            return 1;
        }
        if x >= bar.0 + 78 && x < bar.0 + 144 {
            return 2;
        }
    }
    0
}

fn fill_rect(data: &mut [u8], width: u32, height: u32, rect: (u32, u32, u32, u32), color: [u8; 4]) {
    for y in rect.1..(rect.1 + rect.3).min(height) {
        for x in rect.0..(rect.0 + rect.2).min(width) {
            put_pixel(data, width, x, y, color);
        }
    }
}

fn fill_rounded_rect(
    data: &mut [u8],
    width: u32,
    height: u32,
    rect: (u32, u32, u32, u32),
    radius: u32,
    color: [u8; 4],
) {
    if rect.2 == 0 || rect.3 == 0 {
        return;
    }
    let radius = radius.min(rect.2 / 2).min(rect.3 / 2);
    if radius == 0 {
        fill_rect(data, width, height, rect, color);
        return;
    }
    for row in 0..rect.3 {
        let dy = if row < radius {
            radius - row
        } else if row >= rect.3 - radius {
            row - (rect.3 - radius - 1)
        } else {
            0
        };
        let mut inset = 0;
        if dy > 0 {
            inset = radius;
            for candidate in 0..=radius {
                let dx = radius - candidate;
                if dx * dx + dy * dy <= radius * radius {
                    inset = candidate;
                    break;
                }
            }
        }
        fill_rect(
            data,
            width,
            height,
            (
                rect.0 + inset,
                rect.1 + row,
                rect.2.saturating_sub(inset * 2),
                1,
            ),
            color,
        );
    }
}
fn put_pixel(data: &mut [u8], width: u32, x: u32, y: u32, color: [u8; 4]) {
    let i = (y as usize * width as usize + x as usize) * 4;
    data[i..i + 4].copy_from_slice(&color);
}
fn draw_text(data: &mut [u8], width: u32, height: u32, x: u32, y: u32, text: &str, color: [u8; 4]) {
    for (index, ch) in text.chars().enumerate() {
        let glyph = glyph(ch.to_ascii_lowercase());
        for (row, bits) in glyph.into_iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) != 0 {
                    let px = x + index as u32 * 6 + col;
                    let py = y + row as u32;
                    if px < width && py < height {
                        put_pixel(data, width, px, py, color);
                    }
                }
            }
        }
    }
}
fn glyph(ch: char) -> [u8; 7] {
    match ch {
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        'a' => [0, 14, 1, 15, 17, 19, 13],
        'c' => [0, 14, 17, 16, 16, 17, 14],
        'd' => [1, 1, 15, 17, 17, 19, 13],
        'e' => [0, 14, 17, 31, 16, 17, 14],
        'h' => [16, 16, 30, 17, 17, 17, 17],
        'i' => [4, 0, 12, 4, 4, 4, 14],
        'l' => [12, 4, 4, 4, 4, 4, 14],
        'n' => [0, 30, 17, 17, 17, 17, 17],
        'o' => [0, 14, 17, 17, 17, 17, 14],
        'p' => [0, 30, 17, 17, 30, 16, 16],
        'r' => [0, 22, 25, 16, 16, 16, 16],
        's' => [0, 15, 16, 14, 1, 1, 30],
        't' => [8, 28, 8, 8, 8, 9, 6],
        'u' => [0, 17, 17, 17, 17, 19, 13],
        'w' => [0, 17, 17, 21, 21, 21, 10],
        'x' => [0, 17, 10, 4, 10, 17, 0],
        ' ' => [0; 7],
        _ => [0; 7],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_is_clamped_and_inclusive() {
        let selection = Selection {
            anchor: (-5.0, 4.0),
            current: (12.0, 20.0),
            present: true,
            ..Selection::default()
        };
        assert_eq!(selection_rect(&selection, 10, 10), (0, 4, 10, 6));
    }
    #[test]
    fn toolbar_hit_testing() {
        let bar = scroll_bar_geometry(800, 600);
        assert_eq!(
            scroll_bar_hit(800, 600, ((bar.0 + 20) as f64, (bar.1 + 20) as f64)),
            1
        );
        assert_eq!(
            scroll_bar_hit(800, 600, ((bar.0 + 90) as f64, (bar.1 + 20) as f64)),
            2
        );
    }

    #[test]
    fn scroll_border_never_touches_capture_area() {
        let width = 40;
        let height = 30;
        let rect = (8, 6, 20, 15);
        let mut pixels = vec![0; width as usize * height as usize * 4];
        draw_outside_border(&mut pixels, width, height, rect, [255, 255, 255, 255]);
        for y in rect.1..rect.1 + rect.3 {
            for x in rect.0..rect.0 + rect.2 {
                let index = (y as usize * width as usize + x as usize) * 4;
                assert_eq!(&pixels[index..index + 4], &[0, 0, 0, 0]);
            }
        }
    }

    #[test]
    fn rounded_chip_preserves_transparent_corners() {
        let width = 40;
        let height = 24;
        let mut pixels = vec![0; width as usize * height as usize * 4];
        let color = [12, 34, 56, 242];
        fill_rounded_rect(&mut pixels, width, height, (5, 4, 24, 15), 7, color);
        let corner = (4usize * width as usize + 5) * 4;
        let center = (11usize * width as usize + 17) * 4;
        assert_eq!(&pixels[corner..corner + 4], &[0, 0, 0, 0]);
        assert_eq!(&pixels[center..center + 4], &color);
    }

    #[test]
    fn selection_border_uses_theme_accent() {
        let width = 48;
        let height = 36;
        let mut pixels = vec![0; width as usize * height as usize * 4];
        let theme = Theme {
            accent: [9, 81, 173, 255],
            ..Theme::default()
        };
        draw_selection_border(&mut pixels, width, height, (8, 7, 24, 18), theme);
        let top_left = (7usize * width as usize + 8) * 4;
        assert_eq!(&pixels[top_left..top_left + 4], &theme.accent);
    }
}
