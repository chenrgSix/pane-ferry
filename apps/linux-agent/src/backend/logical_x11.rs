//! Existing Xwayland windows keep their native backing density. The receiver
//! sees logical client pixels; downsampling belongs to the encoding worker.
use super::*;
use anyhow::{bail, Context};
use paneferry_backend_x11::LogicalLayout;

pub struct LogicalX11 {
    source: X11,
    windows: HashMap<u32, Window>,
    densities: HashMap<u32, u32>,
    native_sizes: HashMap<u32, (u32, u32)>,
}

fn density(window: &Window, layout: &LogicalLayout) -> Option<u32> {
    (1..=4).find(|scale| {
        window.width.abs_diff(layout.width * scale) <= 1
            && window.height.abs_diff(layout.height * scale) <= 1
    })
}

impl LogicalX11 {
    pub fn new(source: X11) -> Result<Self> {
        let mut backend = Self {
            source,
            windows: HashMap::new(),
            densities: HashMap::new(),
            native_sizes: HashMap::new(),
        };
        backend.scan()?;
        Ok(backend)
    }
    fn native_size(&self, id: u32, width: u32, height: u32) -> Result<(u32, u32)> {
        anyhow::ensure!(self.windows.contains_key(&id), "unknown window");
        let scale = self.densities.get(&id).copied().unwrap_or(1);
        let width = width.checked_mul(scale).context("invalid dimensions")?;
        let height = height.checked_mul(scale).context("invalid dimensions")?;
        paneferry_protocol::dimensions(width, height)?;
        Ok((width, height))
    }
}

impl WindowBackend for LogicalX11 {
    fn windows(&self) -> &HashMap<u32, Window> {
        &self.windows
    }
    fn prepare_input(&self) {
        self.source.prepare_input()
    }
    fn input_status(&self) -> Option<&'static str> {
        self.source.input_status()
    }
    fn refresh_capture(&self) {
        self.source.refresh_capture()
    }
    fn refresh_window(&self, id: u32) {
        self.source.refresh_window(id)
    }
    fn retry_window_adjustments(&mut self) {
        self.source.retry_window_adjustments()
    }
    fn event_fd(&self) -> std::os::fd::RawFd {
        self.source.event_fd()
    }
    fn existing_window_exists(&self, id: u32) -> bool {
        self.source.existing_window_exists(id)
    }
    fn includes_window(&self, id: u32) -> bool {
        self.windows.contains_key(&id) && self.source.includes_window(id)
    }
    fn scan(&mut self) -> Result<Vec<Window>> {
        let raw = self.source.scan()?;
        let layouts = if raw.is_empty() {
            HashMap::new()
        } else {
            self.source.logical_layout()?
        };
        let mut next = HashMap::new();
        for mut window in raw {
            let id = window.window_id;
            let native_size = (window.width, window.height);
            if let Some(layout) = layouts.get(&id) {
                // The X11 and compositor snapshots can straddle a configure.
                // Only accept a coherent density; retain the previous one in
                // that short interval instead of briefly treating 2x as 1x.
                if let Some(scale) = density(&window, layout) {
                    self.densities.insert(id, scale);
                }
                let scale = self.densities.get(&id).copied().unwrap_or(1);
                window.width = window.width.div_ceil(scale);
                window.height = window.height.div_ceil(scale);
                window.scale_factor = 1;
                window.x = layout.x;
                window.y = layout.y;
                if let Some(presentation) = &mut window.presentation {
                    presentation.x = f64::from(layout.x);
                    presentation.y = f64::from(layout.y);
                    presentation.width = f64::from(window.width);
                    presentation.height = f64::from(window.height);
                }
            }
            window.geometry_revision = self.windows.get(&id).map_or(1, |old| {
                old.geometry_revision
                    + u64::from(
                        old.width != window.width
                            || old.height != window.height
                            || self.native_sizes.get(&id) != Some(&native_size),
                    )
            });
            if self
                .windows
                .get(&id)
                .is_some_and(|old| old.geometry_revision != window.geometry_revision)
            {
                // A compositor can change logical density without damaging the
                // X11 backing pixels. Publish the new mapping even while idle.
                self.source.refresh_window(id);
            }
            self.native_sizes.insert(id, native_size);
            next.insert(id, window);
        }
        self.native_sizes.retain(|id, _| next.contains_key(id));
        self.densities.retain(|id, _| next.contains_key(id));
        self.windows = next;
        Ok(self.windows.values().cloned().collect())
    }
    fn pump_capture_events(&mut self, _: bool) -> Result<()> {
        self.source.pump_capture_events()
    }
    fn capture_pending(&self) -> bool {
        self.source.capture_pending()
    }
    fn capture_needed(&self, id: u32) -> bool {
        self.source.capture_needed(id)
    }
    fn capture(&self, window: &Window) -> Result<Arc<crate::video::Frame>> {
        anyhow::ensure!(
            self.windows.get(&window.window_id) == Some(window),
            "stale window geometry"
        );
        let raw = self
            .source
            .windows
            .get(&window.window_id)
            .context("unknown window")?;
        let image = self.source.capture(raw)?;
        crate::video::Frame::scaled_image(image, window.width, window.height).map(Arc::new)
    }
    fn configure(&self, id: u32, width: u32, height: u32) -> Result<()> {
        let (width, height) = self.native_size(id, width, height)?;
        self.source.configure(id, width, height)
    }
    fn configure_native(&self, id: u32, width: u32, height: u32) -> Result<()> {
        self.source.configure(id, width, height)
    }
    fn focus(&self, id: u32) -> Result<()> {
        self.source.focus(id)
    }
    fn close(&self, id: u32) -> Result<()> {
        self.source.close(id)
    }
    fn pointer(&self, id: u32, x: i32, y: i32, button: u8, pressed: bool) -> Result<()> {
        let logical = self.windows.get(&id).context("unknown window")?;
        if x < 0 || y < 0 || x >= logical.width as i32 || y >= logical.height as i32 {
            if button > 0 && !pressed {
                return self.source.pointer(id, -1, -1, button, pressed);
            }
            bail!("invalid pointer");
        }
        let raw = self.source.windows.get(&id).context("unknown window")?;
        let x = (u64::from(x as u32) * u64::from(raw.width) / u64::from(logical.width)) as i32;
        let y = (u64::from(y as u32) * u64::from(raw.height) / u64::from(logical.height)) as i32;
        self.source.pointer(id, x, y, button, pressed)
    }
    fn key(&self, id: u32, key: u32, pressed: bool) -> Result<()> {
        self.source.key(id, key, pressed)
    }
    fn release_all(&self) {
        self.source.release_all()
    }
    fn copy_text(&mut self, id: u32, request: String) -> Result<()> {
        self.source.copy_text(id, request)
    }
    fn cut_text(&mut self, id: u32, request: String) -> Result<()> {
        self.source.cut_text(id, request)
    }
    fn copy_bundle(&mut self, id: u32, request: String, cut: bool) -> Result<()> {
        self.source.copy_bundle(id, request, cut)
    }
    fn cut_pending(&self) -> bool {
        self.source.cut_pending()
    }
    fn pump_copy(&mut self) -> Result<Option<CopiedText>> {
        self.source.pump_copy()
    }
    fn text_pending(&self) -> bool {
        self.source.text_pending()
    }
    fn cancel_text(&mut self) {
        self.source.cancel_text()
    }
    fn commit_text(&mut self, id: u32, text: String) -> Result<()> {
        self.source.commit_text(id, text)
    }
    fn paste_bundle(&mut self, id: u32, transfer: &str) -> Result<()> {
        self.source.paste_bundle(id, transfer)
    }
    fn pump_text(&mut self) -> Result<()> {
        self.source.pump_text()
    }
}
