//! A physical GNOME monitor is a separate, detachable session. It never owns
//! application processes, window geometry, or the desktop's display settings.
use crate::{
    backend::WindowBackend,
    discovery,
    video::{FrameWake, PipeWireCapture},
};
use anyhow::{bail, Context, Result};
use paneferry_backend_x11::{Backend as X11, CopiedText, DesktopInput};
use paneferry_protocol::{read_line_limit, Window};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    io::{BufReader, Write},
    os::{fd::OwnedFd, unix::net::UnixStream},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

const WINDOW_ID: u32 = 1;

fn pointer_on_monitor(monitor: &Monitor, window: &Window, x: i32, y: i32) -> Result<(i32, i32)> {
    if x < 0 || y < 0 || x >= window.width as i32 || y >= window.height as i32 {
        bail!("invalid pointer coordinates")
    }
    Ok((
        (i64::from(monitor.x)
            + i64::from(x) * i64::from(monitor.logical_width) / i64::from(window.width))
            as i32,
        (i64::from(monitor.y)
            + i64::from(y) * i64::from(monitor.logical_height) / i64::from(window.height))
            as i32,
    ))
}

#[derive(Clone, Debug, Deserialize)]
pub struct Monitor {
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub logical_width: u32,
    pub logical_height: u32,
    pub x: i32,
    pub y: i32,
    pub primary: bool,
}
impl Monitor {
    fn validate(&self) -> Result<()> {
        if !paneferry_protocol::identifier(&self.id)
            || self.name.is_empty()
            || self.name.len() > 128
            || self.logical_width == 0
            || self.logical_height == 0
            || self.logical_width > 8192
            || self.logical_height > 8192
            || i64::from(self.x) + i64::from(self.logical_width) > i64::from(i32::MAX)
            || i64::from(self.y) + i64::from(self.logical_height) > i64::from(i32::MAX)
        {
            bail!("desktop_monitor_unavailable")
        }
        paneferry_protocol::dimensions(self.width, self.height)
            .map_err(|_| anyhow::anyhow!("desktop_monitor_unavailable"))
    }
    pub fn public(&self) -> Value {
        json!({"id":self.id,"name":self.name,"width":self.width,
            "height":self.height,"primary":self.primary})
    }
}

struct Rpc {
    child: Child,
    wire: BufReader<UnixStream>,
}
impl Rpc {
    fn new() -> Result<Self> {
        let (parent, child) = UnixStream::pair()?;
        parent.set_read_timeout(Some(Duration::from_secs(12)))?;
        parent.set_write_timeout(Some(Duration::from_secs(2)))?;
        let input: OwnedFd = child.try_clone()?.into();
        let output: OwnedFd = child.into();
        let child = Command::new("/usr/bin/python3")
            .args(["-I", "-u", "-c", include_str!("desktop_helper.py")])
            .stdin(Stdio::from(input))
            .stdout(Stdio::from(output))
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self {
            child,
            wire: BufReader::new(parent),
        })
    }
    fn call(&mut self, request: Value) -> Result<Value> {
        serde_json::to_writer(self.wire.get_mut(), &request)?;
        self.wire.get_mut().write_all(b"\n")?;
        let line =
            read_line_limit(&mut self.wire, 64 * 1024)?.context("desktop_capture_unavailable")?;
        let answer: Value = serde_json::from_slice(&line)?;
        if let Some(code) = answer["error"].as_str() {
            bail!(
                "{}",
                match code {
                    "desktop_monitor_unavailable" => "desktop_monitor_unavailable",
                    "desktop_locked" => "desktop_locked",
                    "wayland_text_unavailable" => "wayland_text_unavailable",
                    "clipboard_copy_failed" => "clipboard_copy_failed",
                    _ => "desktop_capture_unavailable",
                }
            )
        }
        Ok(answer["result"].clone())
    }
    fn monitors(&mut self) -> Result<Vec<Monitor>> {
        let response = self.call(json!({"op":"list"}))?;
        let monitors: Vec<Monitor> = serde_json::from_value(response["monitors"].clone())?;
        if monitors.len() > 16 {
            bail!("desktop_monitor_unavailable")
        }
        for monitor in &monitors {
            monitor.validate()?;
        }
        Ok(monitors)
    }
}
impl Drop for Rpc {
    fn drop(&mut self) {
        let _ = self.wire.get_mut().shutdown(std::net::Shutdown::Both);
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
pub fn monitors() -> Result<Vec<Monitor>> {
    Rpc::new()?.monitors()
}

struct Stream {
    capture: PipeWireCapture,
    image: Option<Arc<crate::video::Frame>>,
    dirty: bool,
    started: Instant,
}
pub struct Backend {
    rpc: RefCell<Rpc>,
    monitor: Monitor,
    input: Arc<dyn DesktopInput>,
    keymap: X11,
    windows: HashMap<u32, Window>,
    stream: RefCell<Option<Stream>>,
    frame_wake: Arc<FrameWake>,
    pending_pull: bool,
    active: Cell<bool>,
    last_monitor_check: Instant,
    copied: Option<(String, Instant)>,
}
impl Backend {
    pub fn connect(
        monitor: Monitor,
        input: Arc<dyn DesktopInput>,
        desktop: &discovery::Desktop,
    ) -> Result<Self> {
        monitor.validate()?;
        let keymap =
            X11::connect_existing(desktop.display, &desktop.authority, std::process::id())?;
        let title = format!("Ubuntu 桌面 · {}", monitor.name);
        let windows = HashMap::from([(
            WINDOW_ID,
            Window {
                window_id: WINDOW_ID,
                parent_id: None,
                title,
                presentation: None,
                role: "desktop".into(),
                x: 0,
                y: 0,
                width: monitor.width,
                height: monitor.height,
                scale_factor: 1,
                geometry_revision: 1,
                visible: true,
            },
        )]);
        Ok(Self {
            rpc: RefCell::new(Rpc::new()?),
            monitor,
            input,
            keymap,
            windows,
            stream: RefCell::new(None),
            frame_wake: Arc::new(FrameWake::new()?),
            pending_pull: false,
            active: Cell::new(false),
            last_monitor_check: Instant::now(),
            copied: None,
        })
    }
    fn known(&self, id: u32) -> Result<()> {
        if id != WINDOW_ID {
            bail!("unknown window")
        }
        Ok(())
    }
    fn pull_frame(&mut self) -> Result<()> {
        let Some(stream) = self.stream.get_mut().as_mut() else {
            return Ok(());
        };
        if let Some(image) = stream.capture.pull_monitor()? {
            let window = self.windows.get_mut(&WINDOW_ID).unwrap();
            if window.width != image.width || window.height != image.height {
                window.width = image.width;
                window.height = image.height;
                window.geometry_revision += 1;
            }
            stream.image = Some(Arc::new(image));
            stream.dirty = true;
        }
        if stream.image.is_none() && stream.started.elapsed() > Duration::from_secs(8) {
            bail!("desktop_capture_unavailable")
        }
        Ok(())
    }
    fn ready(&self) -> Result<()> {
        match self.input.status() {
            "ready" => Ok(()),
            "pending" => bail!("desktop_authorization_pending"),
            "denied" => bail!("desktop_authorization_denied"),
            "paused" => bail!("desktop_input_paused"),
            _ => bail!("desktop_input_unavailable"),
        }
    }
    fn shortcut(&self, key: u32) -> Result<()> {
        self.ready()?;
        let ctrl = self.keymap.keycode(0xffe3)?;
        let code = self.keymap.keycode(key)?;
        self.input.key(ctrl, true)?;
        std::thread::sleep(Duration::from_millis(12));
        let result = self.input.key(code, true).and_then(|_| {
            std::thread::sleep(Duration::from_millis(12));
            self.input.key(code, false)
        });
        let released = self.input.key(ctrl, false);
        if result.is_err() || released.is_err() {
            self.input.release_all();
        }
        result.and(released)
    }
}
impl WindowBackend for Backend {
    fn windows(&self) -> &HashMap<u32, Window> {
        &self.windows
    }
    fn prepare_input(&self) {
        self.active.set(true);
        self.input.prepare();
    }
    fn input_status(&self) -> Option<&'static str> {
        Some(self.input.status())
    }
    fn refresh_capture(&self) {
        if let Some(stream) = self.stream.borrow_mut().as_mut() {
            stream.dirty = true;
        }
    }
    fn refresh_video(&mut self) {
        if let Some(stream) = self.stream.borrow_mut().as_mut() {
            if stream.capture.refresh_prepared() {
                stream.image = None;
            }
            stream.dirty = true;
            stream.started = Instant::now();
        }
        self.pending_pull = true;
        self.frame_wake.notify();
    }
    fn event_fd(&self) -> std::os::fd::RawFd {
        self.frame_wake.event_fd()
    }
    fn existing_window_exists(&self, id: u32) -> bool {
        id == WINDOW_ID
    }
    fn includes_window(&self, id: u32) -> bool {
        id == WINDOW_ID
    }
    fn scan(&mut self) -> Result<Vec<Window>> {
        if !self.active.get() {
            return Ok(self.windows.values().cloned().collect());
        }
        if self.stream.get_mut().is_none() {
            let response = self
                .rpc
                .get_mut()
                .call(json!({"op":"start","monitor_id":self.monitor.id}))?;
            let node = u32::try_from(
                response["node"]
                    .as_u64()
                    .context("desktop_capture_unavailable")?,
            )?;
            self.stream.replace(Some(Stream {
                capture: PipeWireCapture::new(node, self.frame_wake.clone())?,
                image: None,
                dirty: true,
                started: Instant::now(),
            }));
        }
        if self.last_monitor_check.elapsed() >= Duration::from_secs(2) {
            self.monitor = self
                .rpc
                .get_mut()
                .monitors()?
                .into_iter()
                .find(|monitor| monitor.id == self.monitor.id)
                .context("desktop_monitor_unavailable")?;
            self.last_monitor_check = Instant::now();
        }
        if self
            .stream
            .get_mut()
            .as_ref()
            .is_some_and(|stream| stream.image.is_none())
        {
            self.pull_frame()?;
        }
        Ok(self.windows.values().cloned().collect())
    }
    fn pump_capture_events(&mut self, prepare_pixels: bool) -> Result<()> {
        self.pending_pull |= self.frame_wake.drain();
        if prepare_pixels && self.pending_pull {
            self.pending_pull = false;
            self.pull_frame()?;
        }
        Ok(())
    }
    fn capture_pending(&self) -> bool {
        self.pending_pull
            || self
                .stream
                .borrow()
                .as_ref()
                .is_some_and(|stream| stream.dirty && stream.image.is_some())
    }
    fn capture_needed(&self, id: u32) -> bool {
        id == WINDOW_ID
            && (self.pending_pull
                || self
                    .stream
                    .borrow()
                    .as_ref()
                    .is_some_and(|s| s.dirty && s.image.is_some()))
    }
    fn capture(&self, window: &Window) -> Result<Arc<crate::video::Frame>> {
        self.known(window.window_id)?;
        let mut stream = self.stream.borrow_mut();
        let stream = stream.as_mut().context("frame pending")?;
        let image = stream.image.as_ref().context("frame pending")?;
        if image.width != window.width || image.height != window.height {
            bail!("frame pending")
        }
        stream.dirty = false;
        Ok(Arc::clone(image))
    }
    fn configure(&self, _id: u32, _width: u32, _height: u32) -> Result<()> {
        bail!("desktop_geometry_fixed")
    }
    fn focus(&self, id: u32) -> Result<()> {
        self.known(id)
    }
    fn close(&self, _id: u32) -> Result<()> {
        bail!("desktop_close_unavailable")
    }
    fn pointer(&self, id: u32, x: i32, y: i32, button: u8, pressed: bool) -> Result<()> {
        self.known(id)?;
        let window = &self.windows[&WINDOW_ID];
        let point = pointer_on_monitor(&self.monitor, window, x, y);
        if point.is_err() && button > 0 && !pressed {
            self.input.release_all();
            return Ok(());
        }
        self.ready()?;
        let (x, y) = point?;
        self.input.pointer(x, y, button, pressed)
    }
    fn key(&self, id: u32, keysym: u32, pressed: bool) -> Result<()> {
        self.known(id)?;
        if pressed {
            self.ready()?;
        }
        self.input.key(self.keymap.keycode(keysym)?, pressed)
    }
    fn release_all(&self) {
        self.active.set(false);
        self.input.release_all();
        self.stream.borrow_mut().take();
        let _ = self.rpc.borrow_mut().call(json!({"op":"stop"}));
    }
    fn copy_text(&mut self, window: u32, id: String) -> Result<()> {
        self.copy_bundle(window, id, false)
    }
    fn cut_text(&mut self, window: u32, id: String) -> Result<()> {
        self.copy_bundle(window, id, true)
    }
    fn copy_bundle(&mut self, window: u32, id: String, cut: bool) -> Result<()> {
        self.known(window)?;
        if cut {
            bail!("wayland_cut_unavailable")
        }
        self.ready()?;
        self.rpc
            .get_mut()
            .call(json!({"op":"set_text","text":""}))?;
        self.shortcut(u32::from(b'c'))?;
        self.copied = Some((id, Instant::now()));
        Ok(())
    }
    fn cut_pending(&self) -> bool {
        false
    }
    fn pump_copy(&mut self) -> Result<Option<CopiedText>> {
        let Some((id, started)) = self.copied.take() else {
            return Ok(None);
        };
        if started.elapsed() < Duration::from_millis(100) {
            self.copied = Some((id, started));
            return Ok(None);
        }
        let response = self.rpc.get_mut().call(json!({"op":"get_text"}))?;
        let text = response["text"].as_str().context("clipboard_copy_failed")?;
        if text.is_empty() && started.elapsed() < Duration::from_secs(3) {
            self.copied = Some((id, started));
            return Ok(None);
        }
        paneferry_protocol::validate_text(text)?;
        Ok(Some(CopiedText {
            window_id: WINDOW_ID,
            request_id: id,
            text: text.into(),
            bundle: None,
        }))
    }
    fn text_pending(&self) -> bool {
        self.copied.is_some()
    }
    fn cancel_text(&mut self) {
        self.copied = None;
    }
    fn commit_text(&mut self, window: u32, text: String) -> Result<()> {
        self.known(window)?;
        paneferry_protocol::validate_text(&text)?;
        self.ready()?;
        self.input.sync()?;
        self.rpc
            .get_mut()
            .call(json!({"op":"commit_text","text":text}))?;
        Ok(())
    }
    fn paste_text(&mut self, window: u32, text: String) -> Result<()> {
        self.known(window)?;
        paneferry_protocol::validate_text(&text)?;
        self.ready()?;
        self.rpc
            .get_mut()
            .call(json!({"op":"set_text","text":text}))?;
        std::thread::sleep(Duration::from_millis(16));
        self.shortcut(u32::from(b'v'))
    }
    fn paste_bundle(&mut self, _window: u32, _transfer: &str) -> Result<()> {
        bail!("wayland_binary_clipboard_unavailable")
    }
    fn pump_text(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retina_monitor_and_negative_origin_map_only_inside_the_selected_screen() {
        let monitor = Monitor {
            id: "eDP-1".into(),
            name: "Panel".into(),
            width: 2880,
            height: 1800,
            logical_width: 1440,
            logical_height: 900,
            x: -1440,
            y: 100,
            primary: true,
        };
        let window = Window {
            window_id: WINDOW_ID,
            parent_id: None,
            title: "Desktop".into(),
            presentation: None,
            role: "desktop".into(),
            x: 0,
            y: 0,
            width: 2880,
            height: 1800,
            scale_factor: 1,
            geometry_revision: 1,
            visible: true,
        };
        assert_eq!(
            pointer_on_monitor(&monitor, &window, 0, 0).unwrap(),
            (-1440, 100)
        );
        assert_eq!(
            pointer_on_monitor(&monitor, &window, 1440, 900).unwrap(),
            (-720, 550)
        );
        assert_eq!(
            pointer_on_monitor(&monitor, &window, 2879, 1799).unwrap(),
            (-1, 999)
        );
        assert!(pointer_on_monitor(&monitor, &window, 2880, 0).is_err());
    }
}
