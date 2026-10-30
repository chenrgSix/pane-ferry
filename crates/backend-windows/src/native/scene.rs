//! Compose only validated application surfaces, never a desktop screenshot.
use super::*;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

type Leases = HashMap<String, (u64, String)>;
static LEASES: OnceLock<Mutex<Leases>> = OnceLock::new();
fn leases() -> &'static Mutex<Leases> {
    LEASES.get_or_init(Default::default)
}
pub fn claimed_as_secondary(window: HWND) -> bool {
    let anchor_handle = window.0 as usize;
    leases().lock().unwrap().values().any(|(handle, root)| {
        *handle == window.0 as u64 && !root.ends_with(&format!(":{anchor_handle}"))
    })
}

#[derive(Clone, PartialEq)]
struct Surface {
    target: Target,
    rect: RECT,
}
#[derive(Clone, PartialEq)]
struct Layout {
    rect: RECT,
    width: u32,
    height: u32,
    surfaces: Vec<Surface>,
}
struct Layer {
    capture: capture::Capture,
    pixels: Option<(u32, u32, Vec<u8>)>,
}
pub struct Scene {
    pub revision: u64,
    pub anchor: Target,
    lease: String,
    baseline: HashSet<String>,
    layers: HashMap<String, Layer>,
    layout: Layout,
    presented: Option<Layout>,
    output: Vec<u8>,
    composed_revision: Option<u64>,
    pixels_changed: bool,
    last_composed: std::time::Instant,
    refresh_remaining: u8,
    force_frame: bool,
    pub signal: std::sync::Arc<capture::FrameSignal>,
}

unsafe extern "system" fn enumerate(window: HWND, data: LPARAM) -> BOOL {
    let values = &mut *(data.0 as *mut Vec<HWND>);
    if values.len() < 2048 {
        values.push(window);
    }
    BOOL(1)
}
fn all_windows() -> Result<Vec<HWND>> {
    let mut windows: Vec<HWND> = Vec::new();
    unsafe {
        EnumWindows(
            Some(enumerate),
            LPARAM((&mut windows as *mut Vec<HWND>) as isize),
        )?;
    }
    Ok(windows)
}
pub fn transient(window: HWND) -> bool {
    unsafe {
        let style = GetWindowLongPtrW(window, GWL_STYLE) as u32;
        GetWindowLongPtrW(window, GWL_EXSTYLE) as u32 & WS_EX_NOACTIVATE.0 != 0
            || style & WS_POPUP.0 != 0 && style & WS_CAPTION.0 == 0
    }
}
fn owned_by(window: HWND, roots: &HashSet<usize>) -> bool {
    unsafe {
        let mut owner = GetWindow(window, GW_OWNER).unwrap_or_default();
        for _ in 0..16 {
            if owner.0.is_null() {
                break;
            }
            if roots.contains(&(owner.0 as usize)) {
                return true;
            }
            owner = GetWindow(owner, GW_OWNER).unwrap_or_default();
        }
        if transient(window) {
            let mut gui = GUITHREADINFO {
                cbSize: size_of::<GUITHREADINFO>() as u32,
                ..Default::default()
            };
            let thread = GetWindowThreadProcessId(window, None);
            if GetGUIThreadInfo(thread, &mut gui).is_ok() {
                return roots.contains(&(GetAncestor(gui.hwndMenuOwner, GA_ROOT).0 as usize));
            }
        }
        false
    }
}
fn visible(window: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(window).as_bool()
            || IsIconic(window).as_bool()
            || GetWindowLongPtrW(window, GWL_STYLE) as u32 & WS_CHILD.0 != 0
        {
            return false;
        }
        let mut cloaked = 0u32;
        DwmGetWindowAttribute(window, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4).is_ok()
            && cloaked == 0
    }
}
fn sized(rect: RECT) -> (u32, u32) {
    let width = (rect.right - rect.left).max(1) as f64;
    let height = (rect.bottom - rect.top).max(1) as f64;
    // Keep the application scene bounded even with windows on several monitors.
    let scale = (4096.0 / width)
        .min(4096.0 / height)
        .min((8_000_000.0 / (width * height)).sqrt())
        .min(1.0);
    (
        (width * scale).floor().max(1.0) as u32,
        (height * scale).floor().max(1.0) as u32,
    )
}
impl Scene {
    pub fn new(anchor: &Target) -> Result<Self> {
        let rect = bounds(anchor)?;
        let (width, height) = sized(rect);
        let lease = format!("{}:{}", uuid::Uuid::new_v4(), anchor.handle);
        {
            let mut held = leases().lock().unwrap();
            ensure!(!held.contains_key(&anchor.id), "session_controlled");
            held.insert(anchor.id.clone(), (anchor.handle as u64, lease.clone()));
        }
        let mut scene = Self {
            revision: 0,
            anchor: anchor.clone(),
            lease,
            baseline: HashSet::new(),
            layers: HashMap::new(),
            layout: Layout {
                rect,
                width,
                height,
                surfaces: vec![],
            },
            presented: None,
            output: Vec::new(),
            composed_revision: None,
            pixels_changed: false,
            last_composed: std::time::Instant::now(),
            refresh_remaining: 0,
            force_frame: true,
            signal: std::sync::Arc::default(),
        };
        for window in all_windows()? {
            let mut pid = 0;
            unsafe {
                GetWindowThreadProcessId(window, Some(&mut pid));
            }
            if pid != anchor.pid {
                continue;
            }
            if let Some(target) = unsafe { describe_window(window, title(window)) } {
                scene.baseline.insert(target.id);
            }
        }
        scene.refresh()?;
        Ok(scene)
    }
    pub fn refresh(&mut self) -> Result<()> {
        validate(&self.anchor)?;
        let windows = all_windows()?;
        let mut roots = HashSet::from([self.anchor.handle]);
        // New independent windows from this exact process join the scene. Other
        // already-open projects stay outside it; HWND reuse has a new identity.
        for window in &windows {
            if !visible(*window) || transient(*window) {
                continue;
            }
            let mut pid = 0;
            unsafe {
                GetWindowThreadProcessId(*window, Some(&mut pid));
            }
            if pid != self.anchor.pid {
                continue;
            }
            if let Some(target) = unsafe { describe_window(*window, title(*window)) } {
                if target.started == self.anchor.started && !self.baseline.contains(&target.id) {
                    roots.insert(target.handle);
                }
            }
        }
        let mut surfaces = Vec::new();
        for window in windows {
            if surfaces.len() >= 15 && window != hwnd(&self.anchor) || !visible(window) {
                continue;
            }
            if !roots.contains(&(window.0 as usize)) && !owned_by(window, &roots) {
                continue;
            }
            let Some(target) = (unsafe { describe_window(window, title(window)) }) else {
                continue;
            };
            let rect = bounds(&target)?;
            if target.id != self.anchor.id
                && (rect.right - rect.left < 32 || rect.bottom - rect.top < 16)
            {
                continue;
            }
            let mut held = leases().lock().unwrap();
            if held
                .get(&target.id)
                .is_some_and(|(_, lease)| lease != &self.lease)
            {
                continue;
            }
            held.insert(
                target.id.clone(),
                (target.handle as u64, self.lease.clone()),
            );
            surfaces.push(Surface { target, rect });
        }
        // EnumWindows gives front-to-back order; paint the anchor/back layers first.
        surfaces.reverse();
        let mut rect = bounds(&self.anchor)?;
        for surface in &surfaces {
            rect.left = rect.left.min(surface.rect.left);
            rect.top = rect.top.min(surface.rect.top);
            rect.right = rect.right.max(surface.rect.right);
            rect.bottom = rect.bottom.max(surface.rect.bottom);
        }
        let (width, height) = sized(rect);
        paneferry_protocol::dimensions(width, height)?;
        let live: HashSet<_> = surfaces.iter().map(|s| s.target.id.clone()).collect();
        self.layers.retain(|id, _| live.contains(id));
        leases().lock().unwrap().retain(|id, (_, lease)| {
            lease != &self.lease || id == &self.anchor.id || live.contains(id)
        });
        if rect != self.layout.rect
            || surfaces.len() != self.layout.surfaces.len()
            || surfaces
                .iter()
                .zip(&self.layout.surfaces)
                .any(|(a, b)| a.target.id != b.target.id || a.rect != b.rect)
        {
            self.revision = self
                .revision
                .checked_add(1)
                .context("scene revision exhausted")?;
        }
        self.layout = Layout {
            rect,
            width,
            height,
            surfaces,
        };
        Ok(())
    }
    pub fn geometry(&mut self) -> Result<Geometry> {
        self.refresh()?;
        Ok(self.current_geometry())
    }
    pub fn current_geometry(&self) -> Geometry {
        Geometry {
            content_revision: self.revision,
            title: title(hwnd(&self.anchor)),
            width: self.layout.width,
            height: self.layout.height,
            visible: !self.layout.surfaces.is_empty(),
        }
    }
    #[cfg(test)]
    pub fn frame(&mut self) -> Result<Option<(u32, u32, Vec<u8>)>> {
        self.refresh()?;
        self.frame_prepared()
    }
    pub fn frame_prepared(&mut self) -> Result<Option<(u32, u32, Vec<u8>)>> {
        // Desktop::frame has just refreshed geometry; avoid a third desktop scan.
        if self.layout.surfaces.is_empty() {
            return Ok(None);
        }
        let mut changed = self.pixels_changed || self.composed_revision != Some(self.revision);
        for surface in &self.layout.surfaces {
            if let Err(error) = validate(&surface.target) {
                if surface.target.id == self.anchor.id {
                    return Err(error);
                }
                return Ok(None); // A closing popup does not end its parent session.
            }
            if !self.layers.contains_key(&surface.target.id) {
                let capture =
                    match capture::Capture::new(hwnd(&surface.target), self.signal.clone()) {
                        Ok(capture) => capture,
                        Err(error) if surface.target.id == self.anchor.id => return Err(error),
                        Err(_) => return Ok(None),
                    };
                self.layers.insert(
                    surface.target.id.clone(),
                    Layer {
                        capture,
                        pixels: None,
                    },
                );
            }
            let layer = self.layers.get_mut(&surface.target.id).unwrap();
            let pixels = layer.pixels.get_or_insert_with(|| (0, 0, Vec::new()));
            let captured = match layer.capture.frame_into(&mut pixels.2) {
                Ok(frame) => frame,
                Err(error) if surface.target.id == self.anchor.id => return Err(error),
                Err(_) => return Ok(None),
            };
            if let Some((width, height)) = captured {
                pixels.0 = width;
                pixels.1 = height;
                changed = true;
                // Keep dirtiness if a later popup is still resizing/not ready
                // and this attempt returns before composition.
                self.pixels_changed = true;
            }
            let Some((w, h, _)) = &layer.pixels else {
                return Ok(None);
            };
            if *w != (surface.rect.right - surface.rect.left) as u32
                || *h != (surface.rect.bottom - surface.rect.top) as u32
            {
                return Ok(None);
            }
        }
        if changed {
            self.refresh_remaining = 12;
        }
        if !changed
            && !self.force_frame
            && (self.refresh_remaining == 0
                || self.last_composed.elapsed() < std::time::Duration::from_millis(100))
        {
            return Ok(None);
        }
        if !changed {
            self.refresh_remaining = self.refresh_remaining.saturating_sub(1);
        }
        let cached = !changed
            && self.output.len() == self.layout.width as usize * self.layout.height as usize * 4;
        let _timing = crate::metrics::span(if cached {
            "scene.compose_cached"
        } else if changed {
            "scene.compose_changed"
        } else {
            "scene.compose_refresh"
        });
        let mut output = std::mem::take(&mut self.output);
        output.resize(
            self.layout.width as usize * self.layout.height as usize * 4,
            24,
        );
        if !cached {
            for (index, surface) in self.layout.surfaces.iter().enumerate() {
                let (w, h, pixels) = self.layers[&surface.target.id].pixels.as_ref().unwrap();
                if index == 0 {
                    paint_base(&self.layout, surface.rect, *w, *h, pixels, &mut output);
                } else {
                    paint(&self.layout, surface.rect, *w, *h, pixels, &mut output);
                }
            }
        }
        self.presented = Some(self.layout.clone());
        self.composed_revision = Some(self.revision);
        self.pixels_changed = false;
        self.last_composed = std::time::Instant::now();
        self.force_frame = false;
        Ok(Some((self.layout.width, self.layout.height, output)))
    }
    pub fn recycle(&mut self, pixels: Vec<u8>) {
        self.output = pixels;
    }
    pub fn force_frame(&mut self) {
        self.force_frame = true;
    }
    pub fn input_snapshot(&self) -> InputScene {
        InputScene {
            anchor: self.anchor.clone(),
            layout: self.layout.clone(),
            presented: self.presented.clone(),
        }
    }
    pub fn foreground_for(&self, target: &Target) -> bool {
        self.input_snapshot().foreground_for(target)
    }
}
#[derive(Clone)]
pub(super) struct InputScene {
    pub anchor: Target,
    layout: Layout,
    presented: Option<Layout>,
}
impl InputScene {
    pub fn foreground_for(&self, target: &Target) -> bool {
        let foreground = unsafe { GetForegroundWindow() };
        foreground == hwnd(target)
            || transient(hwnd(target))
                && self
                    .layout
                    .surfaces
                    .iter()
                    .any(|s| hwnd(&s.target) == foreground)
    }
    pub fn route(&self, input: Input) -> Result<(Target, Input)> {
        let layout = self.presented.as_ref().unwrap_or(&self.layout);
        match input {
            Input::Pointer {
                x,
                y,
                button,
                pressed,
            } => {
                ensure!(
                    x >= 0 && y >= 0 && x < layout.width as i32 && y < layout.height as i32,
                    "invalid_request"
                );
                let px = layout.rect.left
                    + ((x as i64 * (layout.rect.right - layout.rect.left) as i64)
                        / layout.width as i64) as i32;
                let py = layout.rect.top
                    + ((y as i64 * (layout.rect.bottom - layout.rect.top) as i64)
                        / layout.height as i64) as i32;
                let surface = layout
                    .surfaces
                    .iter()
                    .rev()
                    .find(|s| {
                        px >= s.rect.left
                            && px < s.rect.right
                            && py >= s.rect.top
                            && py < s.rect.bottom
                    })
                    .context("input_focus_required")?;
                validate(&surface.target)?;
                ensure!(
                    bounds(&surface.target)? == surface.rect,
                    "input_focus_required"
                );
                Ok((
                    surface.target.clone(),
                    Input::Pointer {
                        x: px - surface.rect.left,
                        y: py - surface.rect.top,
                        button,
                        pressed,
                    },
                ))
            }
            Input::Resize { .. } | Input::Close => Ok((self.anchor.clone(), input)),
            _ => {
                let foreground = unsafe { GetForegroundWindow() };
                let target = layout
                    .surfaces
                    .iter()
                    .rev()
                    .find(|s| {
                        hwnd(&s.target) == foreground
                            && unsafe { IsWindowEnabled(foreground).as_bool() }
                    })
                    .or_else(|| {
                        layout
                            .surfaces
                            .iter()
                            .rev()
                            .find(|s| unsafe { IsWindowEnabled(hwnd(&s.target)).as_bool() })
                    })
                    .map(|s| s.target.clone())
                    .unwrap_or_else(|| self.anchor.clone());
                validate(&target)?;
                Ok((target, input))
            }
        }
    }
    pub fn multiple(&self) -> bool {
        self.layout.surfaces.len() > 1
    }
}

impl Drop for Scene {
    fn drop(&mut self) {
        leases()
            .lock()
            .unwrap()
            .retain(|_, (_, lease)| lease != &self.lease);
    }
}

fn paint_base(layout: &Layout, rect: RECT, w: u32, h: u32, source: &[u8], output: &mut [u8]) {
    if rect == layout.rect && w == layout.width && h == layout.height {
        // Fuse canvas initialization with the first layer. Opaque pixels are
        // copied once; translucent borders still blend over the same backdrop.
        blend::over(source, output, true);
    } else {
        output.fill(24);
        for pixel in output.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        paint(layout, rect, w, h, source, output);
    }
}

fn paint(layout: &Layout, rect: RECT, w: u32, h: u32, source: &[u8], output: &mut [u8]) {
    let canvas_w = (layout.rect.right - layout.rect.left) as i64;
    let canvas_h = (layout.rect.bottom - layout.rect.top) as i64;
    let left =
        ((rect.left - layout.rect.left) as i64 * layout.width as i64 / canvas_w).max(0) as usize;
    let top =
        ((rect.top - layout.rect.top) as i64 * layout.height as i64 / canvas_h).max(0) as usize;
    let right = (((rect.right - layout.rect.left) as i64 * layout.width as i64 + canvas_w - 1)
        / canvas_w)
        .min(layout.width as i64) as usize;
    let bottom = (((rect.bottom - layout.rect.top) as i64 * layout.height as i64 + canvas_h - 1)
        / canvas_h)
        .min(layout.height as i64) as usize;
    if right <= left || bottom <= top {
        return;
    }
    if canvas_w == layout.width as i64 && canvas_h == layout.height as i64 {
        // Native-size scenes need no resampling or per-pixel divisions. Most
        // application pixels are opaque; retain blending at translucent borders.
        for y in top..bottom {
            let sy = (layout.rect.top + y as i32 - rect.top) as usize;
            let sx = (layout.rect.left + left as i32 - rect.left) as usize;
            let src = &source[(sy * w as usize + sx) * 4..][..(right - left) * 4];
            let dst = &mut output[(y * layout.width as usize + left) * 4..][..src.len()];
            blend::over(src, dst, false);
        }
        return;
    }
    for y in top..bottom {
        for x in left..right {
            let sx = (layout.rect.left as i64 + x as i64 * canvas_w / layout.width as i64
                - rect.left as i64)
                .clamp(0, w as i64 - 1) as usize;
            let sy = (layout.rect.top as i64 + y as i64 * canvas_h / layout.height as i64
                - rect.top as i64)
                .clamp(0, h as i64 - 1) as usize;
            let src = &source[(sy * w as usize + sx) * 4..][..4];
            let dst = &mut output[(y * layout.width as usize + x) * 4..][..4];
            let alpha = src[3] as u16;
            for c in 0..3 {
                dst[c] =
                    (src[c] as u16 + (dst[c] as u16 * (255 - alpha) + 127) / 255).min(255) as u8;
            }
            dst[3] = 255;
        }
    }
}

#[cfg(test)]
#[path = "menus_test.rs"]
mod tests;
