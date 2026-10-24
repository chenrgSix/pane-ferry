//! Existing GNOME/Wayland windows. The compositor owns window identity and
//! geometry; PipeWire supplies original pixels and the shared portal supplies input.
use crate::{
    backend::WindowBackend,
    process_identity,
    video::{FrameWake, PipeWireCapture},
    ProcessIdentity,
};
use anyhow::{bail, Context, Result};
use paneferry_backend_x11::{Backend as X11, CopiedText, DesktopInput};
use paneferry_protocol::{read_line_limit, Window};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::HashMap,
    io::{BufReader, Write},
    os::{fd::OwnedFd, unix::net::UnixStream},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Deserialize)]
pub struct Info {
    #[serde(default)]
    pub bridge_version: u32,
    pub id: String,
    pub window_id: u32,
    pub pid: u32,
    pub app_id: String,
    pub name: String,
    pub title: String,
    pub native: bool,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub buffer_x: i32,
    pub buffer_y: i32,
    pub buffer_width: u32,
    pub buffer_height: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub parent_id: Option<u32>,
    pub minimized: bool,
    pub scale: f64,
}
impl Info {
    fn validate(&self) -> Result<()> {
        if self.id.parse::<u64>().is_err()
            || self.window_id == 0
            || self.pid == 0
            || self.app_id.len() > 1024
            || self.name.len() > 1024
            || self.title.len() > 2048
            || !self.scale.is_finite()
            || !(1.0..=4.0).contains(&self.scale)
        {
            bail!("wayland_bridge_failed")
        }
        paneferry_protocol::dimensions(self.width, self.height)?;
        if self.buffer_width > 8192
            || self.buffer_height > 8192
            || self.buffer_width < self.width
            || self.buffer_height < self.height
            || self.x < self.buffer_x
            || self.y < self.buffer_y
            || i64::from(self.x) - i64::from(self.buffer_x) + i64::from(self.width)
                > i64::from(self.buffer_width)
            || i64::from(self.y) - i64::from(self.buffer_y) + i64::from(self.height)
                > i64::from(self.buffer_height)
            || i64::from(self.x) + i64::from(self.width) > i64::from(i32::MAX)
            || i64::from(self.y) + i64::from(self.height) > i64::from(i32::MAX)
        {
            bail!("wayland_bridge_failed")
        }
        Ok(())
    }
}
#[derive(Clone)]
pub struct Target {
    pub info: Info,
    pub identity: ProcessIdentity,
    pub catalog_id: Option<String>,
    pub icon: Option<String>,
}
impl Target {
    pub fn id(&self) -> String {
        format!(
            "wayland-{}-{}-{}",
            self.identity.pid, self.identity.started, self.info.window_id
        )
    }
    pub fn public(&self, controlled: bool, session: Option<String>) -> Value {
        json!({"id":self.id(), "app_id":self.catalog_id, "window_id":self.info.window_id,
            "name":self.info.name,"title":self.info.title,"icon_png":self.icon,
            "minimized":self.info.minimized,"controlled":controlled,"session_id":session})
    }
}
struct Rpc {
    child: Child,
    wire: BufReader<UnixStream>,
}
impl Rpc {
    fn new() -> Result<Self> {
        let (parent, child) = UnixStream::pair()?;
        parent.set_read_timeout(Some(Duration::from_secs(15)))?;
        parent.set_write_timeout(Some(Duration::from_secs(2)))?;
        let input: OwnedFd = child.try_clone()?.into();
        let output: OwnedFd = child.into();
        let child = Command::new("/usr/bin/python3")
            .args(["-I", "-u", "-c", include_str!("wayland_helper.py")])
            .stdin(Stdio::from(input))
            .stdout(Stdio::from(output))
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self {
            child,
            wire: BufReader::new(parent),
        })
    }
    fn call(&mut self, value: Value) -> Result<Value> {
        serde_json::to_writer(self.wire.get_mut(), &value)?;
        self.wire.get_mut().write_all(b"\n")?;
        let line =
            read_line_limit(&mut self.wire, 512 * 1024)?.context("wayland_bridge_unavailable")?;
        let answer: Value = serde_json::from_slice(&line)?;
        if let Some(code) = answer["error"].as_str() {
            bail!(
                "{}",
                match code {
                    "window_unavailable" => "unknown window",
                    "session_controlled" => "session already controlled",
                    "desktop_locked" => "desktop_locked",
                    "resolution_limited" => "resolution_limited",
                    "wayland_bridge_unavailable" => "wayland_bridge_unavailable",
                    "wayland_capture_unavailable" => "wayland_capture_unavailable",
                    "wayland_text_unavailable" => "wayland_text_unavailable",
                    _ => "wayland_bridge_failed",
                }
            )
        }
        Ok(answer["result"].clone())
    }
}
impl Drop for Rpc {
    fn drop(&mut self) {
        let _ = self.wire.get_mut().shutdown(std::net::Shutdown::Both);
        // D-Bus unique-name loss also releases compositor leases and streams.
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
pub fn inventory(apps: &[crate::discovery::DesktopApp]) -> Result<Vec<Target>> {
    let value = Rpc::new()?.call(json!({"op":"list"}))?;
    let windows: Vec<Info> = serde_json::from_value(value)?;
    if windows.len() > 128 {
        bail!("wayland_bridge_failed")
    }
    let mut result = Vec::new();
    for info in windows.into_iter().filter(|w| w.native) {
        if info.validate().is_err() {
            continue;
        }
        let Ok(identity) = process_identity(info.pid) else {
            continue;
        };
        if crate::discovery::is_receiver(&identity) {
            continue;
        }
        let app = apps.iter().find(|app| app.matches_desktop_id(&info.app_id));
        result.push(Target {
            info,
            identity,
            catalog_id: app.map(|a| a.id.clone()),
            icon: app.and_then(|a| a.icon_png.clone()),
        });
    }
    Ok(result)
}
struct Stream {
    capture: PipeWireCapture,
    image: Option<Arc<crate::video::Frame>>,
    dirty: bool,
    started: Instant,
}
pub struct Backend {
    rpc: RefCell<Rpc>,
    target: Target,
    input: Arc<dyn DesktopInput>,
    keymap: X11,
    windows: HashMap<u32, Window>,
    infos: HashMap<u32, Info>,
    streams: RefCell<HashMap<u32, Stream>>,
    frame_wake: Arc<FrameWake>,
    pending_pull: bool,
    copied: Option<(u32, String, Instant)>,
}
impl Backend {
    pub fn connect(
        target: Target,
        input: Arc<dyn DesktopInput>,
        desktop: &crate::discovery::Desktop,
    ) -> Result<Self> {
        // GNOME caches loaded extension modules until the next login. An
        // upgraded Agent must not silently use the old text/capture contract.
        if target.info.bridge_version < 2 {
            bail!("wayland_bridge_outdated")
        }
        if process_identity(target.identity.pid).ok().as_ref() != Some(&target.identity) {
            bail!("approved existing process unavailable")
        }
        let mut rpc = Rpc::new()?;
        rpc.call(json!({"op":"acquire", "id":target.info.id, "pid":target.identity.pid}))?;
        rpc.call(json!({"op":"focus", "window_id":target.info.window_id}))?;
        let keymap =
            X11::connect_existing(desktop.display, &desktop.authority, target.identity.pid)?;
        Ok(Self {
            rpc: RefCell::new(rpc),
            target,
            input,
            keymap,
            windows: HashMap::new(),
            infos: HashMap::new(),
            streams: RefCell::new(HashMap::new()),
            frame_wake: Arc::new(FrameWake::new()?),
            pending_pull: false,
            copied: None,
        })
    }
    fn call(&self, op: &str, id: u32) -> Result<Value> {
        self.rpc.borrow_mut().call(json!({"op":op,"window_id":id}))
    }
    fn known(&self, id: u32) -> Result<&Info> {
        self.infos.get(&id).context("unknown window")
    }
    fn pull_frames(&mut self) -> Result<()> {
        let mut streams = self.streams.borrow_mut();
        for (id, info) in &self.infos {
            let Some(stream) = streams.get_mut(id) else {
                continue;
            };
            if let Some(image) = stream.capture.pull(
                [
                    (info.x - info.buffer_x) as u32,
                    (info.y - info.buffer_y) as u32,
                    info.width,
                    info.height,
                ],
                (info.buffer_width, info.buffer_height),
                info.scale,
            )? {
                if let Some(window) = self.windows.get_mut(id) {
                    if window.width != image.width || window.height != image.height {
                        window.width = image.width;
                        window.height = image.height;
                        window.geometry_revision += 1;
                    }
                }
                stream.image = Some(Arc::new(image));
                stream.dirty = true;
            }
            if stream.image.is_none() && stream.started.elapsed() > Duration::from_secs(8) {
                bail!("wayland_capture_unavailable")
            }
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
    fn shortcut(&self, id: u32, key: u32) -> Result<()> {
        self.ready()?;
        self.focus(id)?;
        let control = self.keymap.keycode(0xffe3)?;
        let code = self.keymap.keycode(key)?;
        self.input.key(control, true)?;
        std::thread::sleep(Duration::from_millis(12));
        let result = self.input.key(code, true).and_then(|_| {
            std::thread::sleep(Duration::from_millis(12));
            self.input.key(code, false)
        });
        std::thread::sleep(Duration::from_millis(12));
        let released = self.input.key(control, false);
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
        self.input.prepare();
    }
    fn input_status(&self) -> Option<&'static str> {
        Some(self.input.status())
    }
    fn refresh_capture(&self) {
        for stream in self.streams.borrow_mut().values_mut() {
            stream.dirty = true;
        }
    }
    fn event_fd(&self) -> std::os::fd::RawFd {
        self.frame_wake.event_fd()
    }
    fn existing_window_exists(&self, id: u32) -> bool {
        if process_identity(self.target.identity.pid).ok().as_ref() != Some(&self.target.identity) {
            return false;
        }
        match self.call("list", id) {
            Ok(value) => serde_json::from_value::<Vec<Info>>(value).map_or(true, |windows| {
                windows
                    .iter()
                    .any(|w| w.window_id == id && w.id == self.target.info.id)
            }),
            // Locking the desktop or a transient bridge outage suspends capture;
            // it must not destroy the session or report that Firefox exited.
            Err(_) => true,
        }
    }
    fn includes_window(&self, id: u32) -> bool {
        self.windows.contains_key(&id)
    }
    fn scan(&mut self) -> Result<Vec<Window>> {
        let scope = match self.call("scope", self.target.info.window_id) {
            Err(error) if error.to_string() == "unknown window" => {
                // GNOME disables user extensions while locked. Reacquire only
                // this compositor ID and original process after unlock.
                let mut rpc = Rpc::new()?;
                rpc.call(json!({"op":"acquire", "id":self.target.info.id, "pid":self.target.identity.pid}))?;
                self.streams.get_mut().clear();
                self.rpc.replace(rpc);
                self.call("scope", self.target.info.window_id)?
            }
            result => result?,
        };
        let infos: Vec<Info> = serde_json::from_value(scope)?;
        if infos.len() > 16 {
            bail!("wayland_bridge_failed")
        }
        let mut pixels_total = 0u64;
        for info in &infos {
            info.validate()?;
            let width = (f64::from(info.width) * info.scale).round() as u32;
            let height = (f64::from(info.height) * info.scale).round() as u32;
            paneferry_protocol::dimensions(width, height)
                .map_err(|_| anyhow::anyhow!("resolution_limited"))?;
            pixels_total += u64::from(width) * u64::from(height);
        }
        if pixels_total > 16_000_000 {
            bail!("resolution_limited")
        }
        let mut windows = HashMap::new();
        let mut current = HashMap::new();
        let mut streams = self.streams.borrow_mut();
        for info in infos {
            info.validate()?;
            // Native transients can belong to a helper process, but never another UID.
            process_identity(info.pid)?;
            let id = info.window_id;
            if let std::collections::hash_map::Entry::Vacant(entry) = streams.entry(id) {
                let reply = self.call("capture", id)?;
                let node = u32::try_from(
                    reply["node"]
                        .as_u64()
                        .context("wayland_capture_unavailable")?,
                )?;
                entry.insert(Stream {
                    capture: PipeWireCapture::new(node, self.frame_wake.clone())?,
                    image: None,
                    dirty: true,
                    started: Instant::now(),
                });
            }
            let stream = streams.get_mut(&id).unwrap();
            if info.width > info.max_width || info.height > info.max_height {
                bail!("resolution_limited")
            }
            let pixels = |value: u32| (f64::from(value) * info.scale).round() as u32;
            let (width, height) = stream
                .image
                .as_ref()
                .map(|i| (i.width, i.height))
                .unwrap_or((info.width, pixels(info.height)));
            let mut window = Window {
                window_id: id,
                parent_id: info.parent_id.filter(|_| id != self.target.info.window_id),
                title: info.title.clone(),
                role: if id == self.target.info.window_id {
                    "toplevel"
                } else {
                    "dialog"
                }
                .into(),
                x: 0,
                y: 0,
                width,
                height,
                scale_factor: if info.scale >= 1.5 { 2 } else { 1 },
                geometry_revision: self.windows.get(&id).map_or(1, |w| w.geometry_revision),
                visible: !info.minimized,
            };
            if self
                .windows
                .get(&id)
                .is_some_and(|old| old.width != width || old.height != height)
            {
                window.geometry_revision += 1;
            }
            windows.insert(id, window);
            current.insert(id, info);
        }
        for id in streams
            .keys()
            .filter(|id| !windows.contains_key(id))
            .copied()
            .collect::<Vec<_>>()
        {
            streams.remove(&id);
            let _ = self.call("stop_capture", id);
        }
        self.infos = current;
        self.windows = windows;
        let needs_initial_frame = streams.values().any(|stream| stream.image.is_none());
        drop(streams);
        if needs_initial_frame {
            self.pull_frames()?;
        }
        Ok(self.windows.values().cloned().collect())
    }
    fn pump_capture_events(&mut self, frame_due: bool) -> Result<()> {
        self.pending_pull |= self.frame_wake.drain();
        if frame_due && self.pending_pull {
            self.pending_pull = false;
            self.pull_frames()?;
        }
        Ok(())
    }
    fn capture_pending(&self) -> bool {
        self.pending_pull
            || self
                .streams
                .borrow()
                .values()
                .any(|stream| stream.dirty && stream.image.is_some())
    }
    fn capture_needed(&self, id: u32) -> bool {
        self.pending_pull
            || self
                .streams
                .borrow()
                .get(&id)
                .is_some_and(|s| s.dirty && s.image.is_some())
    }
    fn capture(&self, window: &Window) -> Result<Arc<crate::video::Frame>> {
        let mut streams = self.streams.borrow_mut();
        let stream = streams
            .get_mut(&window.window_id)
            .context("unknown window")?;
        let image = stream.image.as_ref().context("frame pending")?;
        if image.width != window.width || image.height != window.height {
            bail!("frame pending")
        }
        stream.dirty = false;
        Ok(Arc::clone(image))
    }
    fn configure(&self, id: u32, width: u32, height: u32) -> Result<()> {
        let info = self.known(id)?;
        paneferry_protocol::dimensions(width, height)?;
        // Frame pixels and compositor coordinates are different at HiDPI.
        let scale = info.scale;
        self.rpc.borrow_mut().call(json!({"op":"resize","window_id":id,
            "width":(f64::from(width)/scale).round().max(1.0) as u32,"height":(f64::from(height)/scale).round().max(1.0) as u32}))?;
        Ok(())
    }
    fn focus(&self, id: u32) -> Result<()> {
        self.known(id)?;
        self.call("focus", id)?;
        Ok(())
    }
    fn close(&self, id: u32) -> Result<()> {
        self.known(id)?;
        self.call("close", id)?;
        Ok(())
    }
    fn pointer(&self, id: u32, x: i32, y: i32, button: u8, pressed: bool) -> Result<()> {
        let info = self.known(id)?;
        let window = &self.windows[&id];
        if x < 0 || y < 0 || x >= window.width as i32 || y >= window.height as i32 {
            if button > 0 && !pressed {
                self.input.release_all();
                return Ok(());
            }
            bail!("invalid pointer coordinates")
        }
        self.ready()?;
        if pressed && button > 0 {
            self.focus(id)?;
        }
        // The compositor can move the window between frames; re-read position
        // immediately before converting an input into desktop coordinates.
        let scope: Vec<Info> = serde_json::from_value(self.call("scope", id)?)?;
        let actual = scope
            .iter()
            .find(|w| w.id == info.id)
            .context("unknown window")?;
        actual.validate()?;
        let pixels = |n: u32| (f64::from(n) * actual.scale).round() as u32;
        if pixels(actual.width) != window.width || pixels(actual.height) != window.height {
            // A configure can precede its captured frame. Do not click a new
            // layout using old pixels; still release held input during a resize.
            if button > 0 && !pressed {
                self.input.release_all();
                return Ok(());
            }
            bail!("unknown window")
        }
        self.input.pointer(
            actual.x + (f64::from(x) * f64::from(actual.width) / f64::from(window.width)) as i32,
            actual.y + (f64::from(y) * f64::from(actual.height) / f64::from(window.height)) as i32,
            button,
            pressed,
        )
    }
    fn key(&self, id: u32, keysym: u32, pressed: bool) -> Result<()> {
        self.known(id)?;
        if pressed {
            self.ready()?;
            self.focus(id)?;
        }
        self.input.key(self.keymap.keycode(keysym)?, pressed)
    }
    fn release_all(&self) {
        self.input.release_all();
    }
    fn text_pending(&self) -> bool {
        self.copied.is_some()
    }
    fn cancel_text(&mut self) {
        self.copied = None;
    }
    fn pump_text(&mut self) -> Result<()> {
        Ok(())
    }
    fn commit_text(&mut self, window: u32, text: String) -> Result<()> {
        paneferry_protocol::validate_text(&text)?;
        self.known(window)?;
        self.ready()?;
        // EIS and compositor text use different channels. Wait for earlier
        // keys/clicks (e.g. Ctrl+L/Ctrl+A) before committing on the Wayland socket.
        self.input.sync()?;
        self.focus(window)?;
        self.rpc
            .borrow_mut()
            .call(json!({"op":"commit_text","window_id":window,"text":text}))?;
        Ok(())
    }
    fn paste_text(&mut self, window: u32, text: String) -> Result<()> {
        paneferry_protocol::validate_text(&text)?;
        self.known(window)?;
        self.ready()?;
        self.rpc
            .borrow_mut()
            .call(json!({"op":"set_text","window_id":window,"text":text}))?;
        std::thread::sleep(Duration::from_millis(16));
        self.shortcut(window, u32::from(b'v'))
    }
    fn copy_text(&mut self, window: u32, id: String) -> Result<()> {
        self.copy_bundle(window, id, false)
    }
    fn cut_text(&mut self, window: u32, id: String) -> Result<()> {
        self.copy_bundle(window, id, true)
    }
    fn copy_bundle(&mut self, window: u32, id: String, cut: bool) -> Result<()> {
        self.known(window)?;
        // A native cut needs an owner-verified, bounded copy before deletion.
        // GNOME's text bridge cannot yet provide that guarantee. Reject before
        // changing the clipboard or sending Ctrl+X, including oversized text.
        if cut {
            bail!("wayland_cut_unavailable")
        }
        self.ready()?;
        self.rpc
            .borrow_mut()
            .call(json!({"op":"set_text","window_id":window,"text":""}))?;
        self.shortcut(window, u32::from(b'c'))?;
        self.copied = Some((window, id, Instant::now()));
        Ok(())
    }
    fn cut_pending(&self) -> bool {
        false
    }
    fn pump_copy(&mut self) -> Result<Option<CopiedText>> {
        let Some((window, id, started)) = self.copied.take() else {
            return Ok(None);
        };
        if started.elapsed() < Duration::from_millis(100) {
            self.copied = Some((window, id, started));
            return Ok(None);
        }
        let value = self.call("get_text", window)?;
        let text = value["text"].as_str().context("clipboard_copy_failed")?;
        if text.is_empty() && started.elapsed() < Duration::from_secs(3) {
            self.copied = Some((window, id, started));
            return Ok(None);
        }
        paneferry_protocol::validate_text(text).map(|_| {
            Some(CopiedText {
                window_id: window,
                request_id: id,
                text: text.into(),
                bundle: None,
            })
        })
    }
    fn paste_bundle(&mut self, _window: u32, _transfer: &str) -> Result<()> {
        bail!("wayland_binary_clipboard_unavailable")
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        self.input.release_all();
        self.streams.get_mut().clear();
    }
}

/// Do not silently forward a known native instance to another display when the
/// compositor bridge is missing. No profile files, histories or tabs are read.
pub fn native_process_running(app: &crate::discovery::DesktopApp) -> bool {
    use std::{fs, io::Read, path::Path};
    let Some(name) = Path::new(&app.executable).file_name() else {
        return false;
    };
    let Ok(processes) = fs::read_dir("/proc") else {
        return false;
    };
    processes.flatten().any(|p| {
        let Some(pid) = p.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
            return false;
        };
        let Ok(identity) = process_identity(pid) else {
            return false;
        };
        if identity.executable.file_name() != Some(name) {
            return false;
        }
        let Ok(file) = fs::File::open(p.path().join("environ")) else {
            return false;
        };
        let mut env = Vec::new();
        if file.take(128 * 1024).read_to_end(&mut env).is_err() {
            return false;
        }
        env.split(|b| *b == 0)
            .any(|s| s == b"GDK_BACKEND=wayland" || s == b"MOZ_ENABLE_WAYLAND=1")
    })
}

/// Xwayland keeps its existing XComposite capture and clipboard. Only geometry
/// uses the compositor, bound to the same approved XID/PID/start-time identity.
pub struct X11Geometry {
    rpc: RefCell<Rpc>,
    identity: ProcessIdentity,
}
impl X11Geometry {
    fn compositor_id<'a>(windows: &'a Value, pid: u32, xid: u32) -> Option<&'a str> {
        windows.as_array()?.iter().find_map(|window| {
            (window["xwindow"] == xid && window["pid"] == pid)
                .then(|| window["id"].as_str())
                .flatten()
                .filter(|id| id.parse::<u64>().is_ok())
        })
    }

    pub fn connect(
        pid: u32,
        xid: u32,
    ) -> Result<Option<Box<dyn paneferry_backend_x11::DesktopGeometry>>> {
        let mut rpc = Rpc::new()?;
        let values = match rpc.call(json!({"op":"list"})) {
            Ok(value) => value,
            Err(_) => return Ok(None), // Other desktops retain their EWMH path.
        };
        let Some(compositor_id) = Self::compositor_id(&values, pid, xid) else {
            return Ok(None);
        };
        let identity = process_identity(pid)?;
        rpc.call(json!({"op":"acquire","id":compositor_id,"pid":pid}))?;
        Ok(Some(Box::new(Self {
            rpc: RefCell::new(rpc),
            identity,
        })))
    }
    fn scoped(&self, xid: u32) -> Result<bool> {
        if process_identity(self.identity.pid).ok().as_ref() != Some(&self.identity) {
            bail!("approved existing process unavailable")
        }
        let mut result = self.rpc.borrow_mut().call(json!({"op":"scope"}));
        if result
            .as_ref()
            .is_err_and(|e| e.to_string() == "unknown window")
        {
            let mut rpc = Rpc::new()?;
            // Mutter replaces its window ID when an Xwayland window is
            // unmapped and shown again, even when its XID and process survive.
            // A released lease cannot be reacquired using the original ID.
            let windows = rpc.call(json!({"op":"list"}))?;
            let compositor_id =
                Self::compositor_id(&windows, self.identity.pid, xid).context("unknown window")?;
            rpc.call(json!({"op":"acquire","id":compositor_id,"pid":self.identity.pid}))?;
            result = rpc.call(json!({"op":"scope"}));
            if process_identity(self.identity.pid).ok().as_ref() != Some(&self.identity) {
                bail!("approved existing process unavailable")
            }
            self.rpc.replace(rpc);
        }
        Ok(result?
            .as_array()
            .is_some_and(|scope| scope.iter().any(|w| w["xwindow"] == xid)))
    }
}
impl paneferry_backend_x11::DesktopGeometry for X11Geometry {
    fn resize(&self, window: u32, width: u32, height: u32) -> Result<bool> {
        if !self.scoped(window)? {
            return Ok(false);
        }
        self.rpc
            .borrow_mut()
            .call(json!({"op":"resize_x11","xwindow":window,"width":width,"height":height}))?;
        Ok(true)
    }
    fn pointer(
        &self,
        window: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<Option<(i32, i32)>> {
        if !self.scoped(window)? {
            return Ok(None);
        }
        let point = self.rpc.borrow_mut().call(
            json!({"op":"locate_x11","xwindow":window,"x":x,"y":y,"width":width,"height":height}),
        )?;
        let x = i32::try_from(point["x"].as_i64().context("wayland_bridge_failed")?)?;
        let y = i32::try_from(point["y"].as_i64().context("wayland_bridge_failed")?)?;
        Ok(Some((x, y)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reacquire_uses_current_compositor_id_for_the_same_process_and_xid() {
        let windows = json!([
            {"id":"987", "pid":123, "xwindow":42},
            {"id":"654", "pid":124, "xwindow":43}
        ]);
        assert_eq!(X11Geometry::compositor_id(&windows, 123, 42), Some("987"));
        assert_eq!(X11Geometry::compositor_id(&windows, 124, 42), None);
        assert_eq!(X11Geometry::compositor_id(&windows, 123, 43), None);
        assert_eq!(
            X11Geometry::compositor_id(&json!([{"id":"bad", "pid":123, "xwindow":42}]), 123, 42),
            None
        );
    }
    #[test]
    fn validates_frame_offsets_without_overflow_or_clipping() {
        let info: Info = serde_json::from_value(json!({
            "id":"42", "window_id":1, "pid":123, "app_id":"fixture.desktop", "name":"Fixture", "title":"Fixture",
            "native":true, "x":10, "y":20, "width":700, "height":450,
            "buffer_x":-16, "buffer_y":-6, "buffer_width":752, "buffer_height":502,
            "max_width":1548, "max_height":848, "parent_id":null, "minimized":false, "scale":2.0
        })).unwrap();
        assert!(info.validate().is_ok());
        let mut bad = info.clone();
        bad.buffer_x = i32::MIN;
        assert!(bad.validate().is_err());
        bad = info.clone();
        bad.buffer_height = 451;
        assert!(bad.validate().is_err());
        bad = info.clone();
        bad.x = i32::MAX;
        bad.buffer_x = i32::MAX - 26;
        assert!(bad.validate().is_err());
        bad = info.clone();
        bad.scale = f64::NAN;
        assert!(bad.validate().is_err());
    }
}
