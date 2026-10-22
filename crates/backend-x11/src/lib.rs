use anyhow::{bail, Context, Result};
pub use paneferry_capture::Image as CapturedImage;
use paneferry_protocol::Window;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    os::unix::net::UnixStream,
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
};
use x11rb::{
    connection::Connection,
    protocol::{composite::ConnectionExt as _, xproto::*, xtest::ConnectionExt as _},
    rust_connection::{DefaultStream, RustConnection},
};
pub mod clipboard_store;
mod controller_copy;
mod controller_text;
mod damage;
mod inventory;
mod related_windows;
mod shared_capture;
pub use controller_copy::CopiedText;
use controller_copy::CopyTransfer;
use controller_text::TextTransfer;
pub use inventory::{desktop_windows, DesktopWindow};
/// Agent-owned desktop input. Capture and clipboard selections keep their own
/// X11 connection; only validated input crosses this shared authorization route.
pub trait DesktopInput: Send + Sync {
    fn prepare(&self);
    fn status(&self) -> &'static str;
    fn pointer(&self, x: i32, y: i32, button: u8, pressed: bool) -> Result<()>;
    fn key(&self, code: u8, pressed: bool) -> Result<()>;
    fn sync(&self) -> Result<()>;
    fn release_all(&self);
}
/// Optional compositor geometry for an already approved Xwayland window.
pub trait DesktopGeometry: Send {
    fn resize(&self, window: u32, width: u32, height: u32) -> Result<bool>;
    fn pointer(
        &self,
        window: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<Option<(i32, i32)>>;
}
pub struct Backend {
    conn: RustConnection,
    root: u32,
    atoms: HashMap<String, u32>,
    pub windows: HashMap<u32, Window>,
    pressed: RefCell<HashSet<(u8, bool)>>,
    process_id: Option<u32>,
    root_window: Option<u32>,
    existing_display: Option<u16>,
    window_scope: Option<related_windows::Scope>,
    text_transfer: Option<TextTransfer>,
    copy_transfer: Option<CopyTransfer>,
    scale_factor: u8,
    shared_capture: RefCell<shared_capture::SharedCapture>,
    damage: RefCell<damage::Damage>,
    desktop_input: Option<Arc<dyn DesktopInput>>,
    desktop_geometry: Option<Box<dyn DesktopGeometry>>,
}
impl Backend {
    pub fn is_xwayland(&self) -> Result<bool> {
        Ok(self.conn.query_extension(b"XWAYLAND")?.reply()?.present)
    }
    pub fn set_desktop_input(&mut self, input: Arc<dyn DesktopInput>) {
        self.desktop_input = Some(input);
    }
    pub fn set_desktop_geometry(&mut self, geometry: Box<dyn DesktopGeometry>) {
        self.desktop_geometry = Some(geometry);
    }
    pub fn prepare_input(&self) {
        if let Some(input) = &self.desktop_input {
            input.prepare();
        }
    }
    pub fn input_status(&self) -> Option<&'static str> {
        self.desktop_input.as_ref().map(|input| input.status())
    }
    fn require_input(&self) -> Result<()> {
        if let Some(input) = &self.desktop_input {
            match input.status() {
                "ready" => {}
                "pending" => bail!("desktop_authorization_pending"),
                "denied" => bail!("desktop_authorization_denied"),
                "paused" => bail!("desktop_input_paused"),
                _ => bail!("desktop_input_unavailable"),
            }
        }
        Ok(())
    }
    /// A fresh controller needs a snapshot even if the application stayed idle
    /// while disconnected and its last damage was already acknowledged.
    pub fn refresh_capture(&self) {
        self.damage.borrow_mut().refresh();
    }
    /// Readiness only; ownership remains with this backend for the controller lease.
    pub fn event_fd(&self) -> std::os::fd::RawFd {
        use std::os::fd::AsRawFd;
        self.conn.stream().as_raw_fd()
    }
    pub fn connect(display: &str) -> Result<Self> {
        let (conn, n) = x11rb::connect(Some(display))?;
        Self::from_connection(conn, n, None)
    }
    pub fn connect_existing(display: u16, authority: &Path, process_id: u32) -> Result<Self> {
        let conn = authenticated_connection(display, authority)?;
        conn.composite_query_version(0, 4)?.reply()?;
        let mut backend = Self::from_connection(conn, 0, Some(process_id))?;
        backend.existing_display = Some(display);
        Ok(backend)
    }
    /// Restrict an explicitly selected existing window, including its dialogs.
    pub fn restrict_to_window(&mut self, window: u32) -> Result<()> {
        if window == 0 || self.client_pid(window).ok() != self.process_id {
            bail!("unknown window")
        }
        let attributes = self.conn.get_window_attributes(window)?.reply()?;
        let ids = self.window_ids()?;
        self.window_scope = Some(related_windows::Scope::new(
            self.existing_display.context("missing desktop")?,
            self.process_id.context("missing process")?,
            window,
            &self.window_candidates(&ids),
        )?);
        self.root_window = Some(window);
        if attributes.map_state != MapState::VIEWABLE {
            self.conn.map_window(window)?.check()?;
            self.conn.flush()?;
        }
        Ok(())
    }
    pub fn existing_window_exists(&self, window: u32) -> bool {
        self.matches_process(window)
            && self
                .conn
                .get_window_attributes(window)
                .ok()
                .and_then(|request| request.reply().ok())
                .is_some()
    }
    fn from_connection(conn: RustConnection, n: usize, process_id: Option<u32>) -> Result<Self> {
        let root = conn.setup().roots[n].root;
        let mut atoms = HashMap::new();
        for name in [
            "_NET_CLIENT_LIST",
            "_NET_WM_NAME",
            "UTF8_STRING",
            "WM_TRANSIENT_FOR",
            "WM_PROTOCOLS",
            "WM_DELETE_WINDOW",
            "_NET_ACTIVE_WINDOW",
            "_NET_WM_STATE",
            "_NET_WM_STATE_MAXIMIZED_HORZ",
            "_NET_WM_STATE_MAXIMIZED_VERT",
            "_NET_MOVERESIZE_WINDOW",
            "_NET_WM_PID",
            "_NET_SUPPORTING_WM_CHECK",
            "WM_CLASS",
            "CLIPBOARD",
            "TARGETS",
            "INCR",
            "image/png",
            "text/uri-list",
            "x-special/gnome-copied-files",
            "application/x-kde-cutselection",
            "TEXT",
            "TIMESTAMP",
            "text/plain;charset=utf-8",
            "text/plain",
            "_LAN_REMOTEAPP_TEXT_TIME",
            "_LAN_REMOTEAPP_CLIPBOARD",
        ] {
            atoms.insert(
                name.into(),
                conn.intern_atom(false, name.as_bytes())?.reply()?.atom,
            );
        }
        let shared_capture = RefCell::new(shared_capture::SharedCapture::new(&conn));
        let damage = RefCell::new(damage::Damage::new(&conn));
        Ok(Self {
            conn,
            root,
            atoms,
            windows: HashMap::new(),
            pressed: RefCell::new(HashSet::new()),
            process_id,
            root_window: None,
            existing_display: None,
            window_scope: None,
            text_transfer: None,
            copy_transfer: None,
            scale_factor: 1,
            shared_capture,
            damage,
            desktop_input: None,
            desktop_geometry: None,
        })
    }
    pub fn set_scale_factor(&mut self, scale: u8) -> Result<()> {
        paneferry_protocol::validate_scale(scale)?;
        self.scale_factor = scale;
        Ok(())
    }
    fn atom(&self, n: &str) -> u32 {
        self.atoms[n]
    }
    fn prop(&self, w: u32, name: &str, ty: AtomEnum) -> Result<GetPropertyReply> {
        Ok(self
            .conn
            .get_property(false, w, self.atom(name), ty, 0, 4096)?
            .reply()?)
    }
    fn belongs_to_process(&self, mut id: u32) -> bool {
        if let Some(scope) = &self.window_scope {
            return self.client_pid(id).is_ok_and(|pid| scope.contains(id, pid));
        }
        if !self.matches_process(id) {
            return false;
        }
        let Some(root) = self.root_window else {
            return true;
        };
        let mut seen = HashSet::new();
        for _ in 0..16 {
            if id == root {
                return true;
            }
            if !seen.insert(id) {
                return false;
            }
            let Some(parent) = self
                .prop(id, "WM_TRANSIENT_FOR", AtomEnum::WINDOW)
                .ok()
                .and_then(|p| p.value32().and_then(|mut values| values.next()))
            else {
                return false;
            };
            id = parent;
        }
        false
    }
    fn matches_process(&self, mut id: u32) -> bool {
        let Some(process_id) = self.process_id else {
            return true;
        };
        // Discovered windows use the X server's peer credentials. This also
        // handles PID namespaces where _NET_WM_PID is a container-local number.
        if self.root_window.is_some() {
            return self.client_pid(id).ok() == Some(process_id);
        }
        let mut visited = HashSet::new();
        for _ in 0..16 {
            if !visited.insert(id) {
                return false;
            }
            if let Some(pid) = self
                .prop(id, "_NET_WM_PID", AtomEnum::CARDINAL)
                .ok()
                .and_then(|p| p.value32().and_then(|mut v| v.next()))
            {
                return pid == process_id;
            }
            match self
                .prop(id, "WM_TRANSIENT_FOR", AtomEnum::WINDOW)
                .ok()
                .and_then(|p| p.value32().and_then(|mut v| v.next()))
            {
                Some(parent) if parent != self.root => id = parent,
                _ => return false,
            }
        }
        false
    }
    fn window_ids(&self) -> Result<Vec<u32>> {
        let mut ids: Vec<_> = self
            .prop(self.root, "_NET_CLIENT_LIST", AtomEnum::WINDOW)?
            .value32()
            .into_iter()
            .flatten()
            .take(512)
            .collect();
        ids.extend(self.windows.keys().copied());
        for id in self
            .conn
            .query_tree(self.root)?
            .reply()?
            .children
            .into_iter()
            .take(512)
        {
            if self
                .conn
                .get_window_attributes(id)
                .ok()
                .and_then(|r| r.reply().ok())
                .is_some_and(|a| a.override_redirect)
            {
                ids.push(id);
            }
        }
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }
    fn window_candidates(&self, ids: &[u32]) -> Vec<related_windows::Candidate> {
        ids.iter()
            .filter_map(|&id| {
                let attributes = self.conn.get_window_attributes(id).ok()?.reply().ok()?;
                let pid = self.client_pid(id).ok()?;
                let parent = self
                    .prop(id, "WM_TRANSIENT_FOR", AtomEnum::WINDOW)
                    .ok()
                    .and_then(|p| p.value32().and_then(|mut v| v.next()))
                    .filter(|p| *p != self.root && *p != id);
                Some(related_windows::Candidate {
                    id,
                    pid,
                    parent,
                    visible: attributes.map_state == MapState::VIEWABLE,
                })
            })
            .collect()
    }
    pub fn includes_window(&self, id: u32) -> bool {
        self.windows.contains_key(&id) && self.belongs_to_process(id)
    }
    pub fn scan(&mut self) -> Result<Vec<Window>> {
        let wm = self
            .prop(self.root, "_NET_SUPPORTING_WM_CHECK", AtomEnum::WINDOW)
            .ok()
            .and_then(|p| p.value32().and_then(|mut ids| ids.next()));
        let wm_pid = if self.process_id.is_none() {
            wm.and_then(|id| self.client_pid(id).ok())
        } else {
            None
        };
        let ids = self.window_ids()?;
        let candidates = if self.window_scope.is_some() {
            self.window_candidates(&ids)
        } else {
            Vec::new()
        };
        if let Some(scope) = &mut self.window_scope {
            scope.update(&candidates);
        }
        // Unmapping/withdrawing a window is not destruction. Keep known XIDs
        // until the server confirms they no longer exist, including cached menus.
        let mut out = Vec::new();
        for id in ids {
            if !self.belongs_to_process(id) {
                continue;
            }
            if Some(id) == wm || wm_pid.is_some() && self.client_pid(id).ok() == wm_pid {
                continue;
            }
            if out.len() >= 16 {
                break;
            }
            let Ok(a) = self.conn.get_window_attributes(id)?.reply() else {
                continue;
            };
            // Unmapped toolkit helpers are not application windows. Retain a
            // hidden XID only after it has actually been shown to this client.
            if a.map_state != MapState::VIEWABLE && !self.windows.contains_key(&id) {
                continue;
            }
            let Ok(g) = self.conn.get_geometry(id)?.reply() else {
                continue;
            };
            if paneferry_protocol::dimensions(g.width.into(), g.height.into()).is_err() {
                // A restored maximized app can initially exceed the frame
                // budget (the virtual desktop is larger than one allowed frame).
                // Normalize it instead of silently losing its only window.
                let width = f64::from(g.width.min(4096));
                let height = f64::from(g.height.min(2160));
                let fit = (8_000_000.0 / (width * height)).sqrt().min(1.0);
                self.resize_window(
                    id,
                    (width * fit) as u32,
                    (height * fit) as u32,
                    a.override_redirect,
                )?;
                continue;
            }
            let pos = self
                .conn
                .translate_coordinates(id, self.root, 0, 0)?
                .reply()?;
            let title = self
                .prop(id, "_NET_WM_NAME", AtomEnum::ANY)
                .ok()
                .map(|p| {
                    String::from_utf8_lossy(&p.value)
                        .chars()
                        .take(512)
                        .collect::<String>()
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Linux Application".into());
            let parent_id = self
                .window_scope
                .as_ref()
                .and_then(|s| s.parent(id))
                .or_else(|| {
                    self.prop(id, "WM_TRANSIENT_FOR", AtomEnum::WINDOW)
                        .ok()
                        .and_then(|p| p.value32().and_then(|mut v| v.next()))
                        .filter(|id| self.belongs_to_process(*id))
                });
            let mut w = Window {
                window_id: id,
                parent_id,
                title,
                role: if a.override_redirect {
                    "popup"
                } else if parent_id.is_some() {
                    "dialog"
                } else {
                    "toplevel"
                }
                .into(),
                x: pos.dst_x.into(),
                y: pos.dst_y.into(),
                width: g.width.into(),
                height: g.height.into(),
                scale_factor: self.scale_factor,
                geometry_revision: 1,
                visible: a.map_state == MapState::VIEWABLE,
            };
            if let Some(old) = self.windows.get(&id) {
                w.geometry_revision = old.geometry_revision
                    + u64::from(old.width != w.width || old.height != w.height);
            }
            out.push(w);
        }
        if self.windows.values().any(|old| {
            old.visible
                && !out
                    .iter()
                    .any(|w| w.window_id == old.window_id && w.visible)
        }) {
            self.release_all();
        }
        self.windows = out.iter().cloned().map(|w| (w.window_id, w)).collect();
        self.damage.borrow_mut().sync(&self.conn, &self.windows)?;
        Ok(out)
    }
    fn next_event(&self) -> Result<Option<x11rb::protocol::Event>> {
        for _ in 0..256 {
            match self.conn.poll_for_event()? {
                Some(x11rb::protocol::Event::DamageNotify(event)) => {
                    self.damage.borrow_mut().notify(&event)
                }
                event => return Ok(event),
            }
        }
        Ok(None)
    }
    pub fn pump_capture_events(&self) -> Result<()> {
        // A transfer's event handler must see its own selection/property events.
        if self.text_transfer.is_some() || self.copy_transfer.is_some() {
            return Ok(());
        }
        for _ in 0..256 {
            if self.next_event()?.is_none() {
                break;
            }
        }
        Ok(())
    }
    pub fn capture_pending(&self) -> bool {
        self.damage.borrow().pending()
    }
    pub fn capture_needed(&self, window: u32) -> bool {
        self.damage.borrow().needed(window)
    }
    pub fn capture(&self, w: &Window) -> Result<CapturedImage> {
        let result = self.capture_image(w);
        if result.is_err() {
            self.damage.borrow_mut().retry(w.window_id);
        }
        result
    }
    fn capture_image(&self, w: &Window) -> Result<CapturedImage> {
        self.known(w.window_id)?;
        self.damage
            .borrow_mut()
            .acknowledge(&self.conn, w.window_id)?;
        // A compositor redirects existing desktop windows. Naming that pixmap
        // lets us capture this window even when another window covers it.
        let pixmap = if self.process_id.is_some() {
            let id = self.conn.generate_id()?;
            self.conn
                .composite_name_window_pixmap(w.window_id, id)?
                .check()
                .ok()
                .map(|_| id)
        } else {
            None
        };
        let image = (|| -> Result<_> {
            let drawable = pixmap.unwrap_or(w.window_id);
            if let Some(image) = self
                .shared_capture
                .borrow_mut()
                .capture(&self.conn, drawable, w.width, w.height)
            {
                return Ok(image);
            }
            let image = self
                .conn
                .get_image(
                    ImageFormat::Z_PIXMAP,
                    drawable,
                    0,
                    0,
                    w.width as u16,
                    w.height as u16,
                    u32::MAX,
                )?
                .reply()?;
            Ok((image.depth, image.visual, image.data))
        })();
        if let Some(pixmap) = pixmap {
            self.conn.free_pixmap(pixmap)?;
        }
        let (depth, visual_id, data) = image?;
        let visual_id = if visual_id == 0 {
            self.conn
                .get_window_attributes(w.window_id)?
                .reply()?
                .visual
        } else {
            visual_id
        };
        let setup = self.conn.setup();
        let f = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == depth)
            .ok_or_else(|| anyhow::anyhow!("missing pixel format"))?;
        let visual = setup
            .roots
            .iter()
            .flat_map(|s| &s.allowed_depths)
            .flat_map(|d| &d.visuals)
            .find(|v| v.visual_id == visual_id)
            .ok_or_else(|| anyhow::anyhow!("missing visual"))?;
        Ok(CapturedImage {
            width: w.width,
            height: w.height,
            data,
            bpp: f.bits_per_pixel,
            pad: f.scanline_pad,
            little: setup.image_byte_order == ImageOrder::LSB_FIRST,
            masks: (visual.red_mask, visual.green_mask, visual.blue_mask),
        })
    }
    fn known(&self, id: u32) -> Result<()> {
        if !self.windows.contains_key(&id) || !self.belongs_to_process(id) {
            bail!("unknown window")
        };
        Ok(())
    }
    pub fn configure(&self, id: u32, w: u32, h: u32) -> Result<()> {
        self.known(id)?;
        paneferry_protocol::dimensions(w, h)?;
        if let Some(geometry) = &self.desktop_geometry {
            if geometry.resize(id, w, h)? {
                return Ok(());
            }
        }
        self.resize_window(id, w, h, self.windows[&id].role == "popup")
    }
    fn resize_window(&self, id: u32, w: u32, h: u32, popup: bool) -> Result<()> {
        if !popup {
            // Managed/maximized windows must be resized through their WM.
            // Direct ConfigureWindow is commonly ignored for restored maximized Firefox.
            for event in [
                ClientMessageEvent::new(
                    32,
                    id,
                    self.atom("_NET_WM_STATE"),
                    [
                        0,
                        self.atom("_NET_WM_STATE_MAXIMIZED_HORZ"),
                        self.atom("_NET_WM_STATE_MAXIMIZED_VERT"),
                        2,
                        0,
                    ],
                ),
                ClientMessageEvent::new(
                    32,
                    id,
                    self.atom("_NET_MOVERESIZE_WINDOW"),
                    [(1 << 10) | (1 << 11) | (2 << 12), 0, 0, w, h],
                ),
            ] {
                self.conn
                    .send_event(
                        false,
                        self.root,
                        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                        event,
                    )?
                    .check()?;
            }
        } else {
            self.conn
                .configure_window(id, &ConfigureWindowAux::new().width(w).height(h))?
                .check()?;
        }
        self.conn.flush()?;
        Ok(())
    }
    pub fn focus(&self, id: u32) -> Result<()> {
        self.known(id)?;
        if !self.windows[&id].visible {
            bail!("window is hidden")
        }
        if self.process_id.is_some() {
            let event = ClientMessageEvent::new(
                32,
                id,
                self.atom("_NET_ACTIVE_WINDOW"),
                [2, x11rb::CURRENT_TIME, 0, 0, 0],
            );
            self.conn.send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )?;
        }
        self.conn
            .set_input_focus(InputFocus::PARENT, id, x11rb::CURRENT_TIME)?
            .check()?;
        self.conn.flush()?;
        Ok(())
    }
    fn focus_for_input(&self, id: u32) -> Result<()> {
        self.known(id)?;
        if !self.windows[&id].visible {
            bail!("window is hidden")
        }
        let mut focused = self.conn.get_input_focus()?.reply()?.focus;
        for _ in 0..16 {
            if focused == id {
                return Ok(());
            }
            if focused <= 1 || focused == self.root {
                break;
            }
            match self.conn.query_tree(focused)?.reply() {
                Ok(tree) => focused = tree.parent,
                Err(_) => break,
            }
        }
        self.focus(id)
    }
    pub fn close(&self, id: u32) -> Result<()> {
        self.known(id)?;
        let msg = ClientMessageEvent::new(
            32,
            id,
            self.atom("WM_PROTOCOLS"),
            [self.atom("WM_DELETE_WINDOW"), x11rb::CURRENT_TIME, 0, 0, 0],
        );
        self.conn
            .send_event(false, id, EventMask::NO_EVENT, msg)?
            .check()?;
        self.conn.flush()?;
        Ok(())
    }
    pub fn pointer(&self, id: u32, x: i32, y: i32, button: u8, pressed: bool) -> Result<()> {
        self.known(id)?;
        let w = &self.windows[&id];
        if x < 0 || y < 0 || x >= w.width as i32 || y >= w.height as i32 || button > 7 {
            if (1..=7).contains(&button) && !pressed {
                self.release_all();
                return Ok(());
            }
            bail!("invalid pointer")
        };
        if pressed || button == 0 {
            self.require_input()?;
        }
        if button != 0 && pressed {
            self.focus_for_input(id)?;
            if let Some(scope) = &self.window_scope {
                scope.record_input(id);
            }
        }
        let point = self
            .conn
            .translate_coordinates(id, self.root, x as i16, y as i16)?
            .reply()?;
        if let Some(input) = &self.desktop_input {
            if let Some(geometry) = &self.desktop_geometry {
                let window = &self.windows[&id];
                let point = geometry.pointer(id, x, y, window.width, window.height);
                if point.is_err() && button > 0 && !pressed {
                    input.release_all();
                    return Ok(());
                }
                if let Some((px, py)) = point? {
                    return input.pointer(px, py, button, pressed);
                }
            }
            return input.pointer(point.dst_x.into(), point.dst_y.into(), button, pressed);
        }
        // XWarpPointer only moves Xwayland's internal pointer. Button events
        // go through the compositor's input-emulation portal, so motion must
        // use the same XTEST route or clicks land at the old desktop position.
        self.conn
            .xtest_fake_input(
                MOTION_NOTIFY_EVENT,
                0,
                x11rb::CURRENT_TIME,
                self.root,
                point.dst_x,
                point.dst_y,
                0,
            )?
            .check()?;
        if button != 0 {
            if pressed {
                self.pressed.borrow_mut().insert((button, false));
            } else {
                self.pressed.borrow_mut().remove(&(button, false));
            }
            self.conn
                .xtest_fake_input(
                    if pressed {
                        BUTTON_PRESS_EVENT
                    } else {
                        BUTTON_RELEASE_EVENT
                    },
                    button,
                    x11rb::CURRENT_TIME,
                    self.root,
                    0,
                    0,
                    0,
                )?
                .check()?;
        }
        self.conn.flush()?;
        Ok(())
    }
    pub fn keycode(&self, keysym: u32) -> Result<u8> {
        let setup = self.conn.setup();
        let min = setup.min_keycode;
        let max = setup.max_keycode;
        let map = self
            .conn
            .get_keyboard_mapping(min, max - min + 1)?
            .reply()?;
        let code = map
            .keysyms
            .chunks(map.keysyms_per_keycode as usize)
            .position(|syms| syms.contains(&keysym))
            .map(|i| min + i as u8)
            .ok_or_else(|| anyhow::anyhow!("keysym unavailable in server layout"))?;
        Ok(code)
    }
    pub fn key(&self, id: u32, keysym: u32, pressed: bool) -> Result<()> {
        self.known(id)?;
        if pressed {
            self.require_input()?;
        }
        if pressed {
            self.focus_for_input(id)?;
            if let Some(scope) = &self.window_scope {
                scope.record_input(id);
            }
        }
        let code = self.keycode(keysym)?;
        if let Some(input) = &self.desktop_input {
            return input.key(code, pressed);
        }
        if pressed {
            self.pressed.borrow_mut().insert((code, true));
        } else {
            self.pressed.borrow_mut().remove(&(code, true));
        }
        self.conn
            .xtest_fake_input(
                if pressed {
                    KEY_PRESS_EVENT
                } else {
                    KEY_RELEASE_EVENT
                },
                code,
                x11rb::CURRENT_TIME,
                self.root,
                0,
                0,
                0,
            )?
            .check()?;
        self.conn.flush()?;
        Ok(())
    }
    pub fn release_all(&self) {
        if let Some(input) = &self.desktop_input {
            input.release_all();
            return;
        }
        for (code, key) in self.pressed.borrow_mut().drain() {
            let _ = self.conn.xtest_fake_input(
                if key {
                    KEY_RELEASE_EVENT
                } else {
                    BUTTON_RELEASE_EVENT
                },
                code,
                x11rb::CURRENT_TIME,
                self.root,
                0,
                0,
                0,
            );
        }
        let _ = self.conn.flush();
    }
}

/// Authenticate to one local X server without mutating global XAUTHORITY.
fn authenticated_connection(display: u16, authority: &Path) -> Result<RustConnection> {
    // Read only the approved server's cookie. Never change process-global
    // XAUTHORITY while other session threads are connecting.
    let auth = Command::new("/usr/bin/xauth")
        .arg("-f")
        .arg(authority)
        .arg("list")
        .arg(format!(":{display}"))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !auth.status.success() {
        bail!("cannot read approved X11 authority")
    }
    let cookie = String::from_utf8(auth.stdout)?;
    let encoded = cookie
        .lines()
        .find_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            (fields.len() == 3 && fields[1] == "MIT-MAGIC-COOKIE-1").then(|| fields[2])
        })
        .context("approved X11 cookie unavailable")?;
    if encoded.len() != 32 || !encoded.bytes().all(|c| c.is_ascii_hexdigit()) {
        bail!("invalid X11 cookie")
    }
    let cookie = (0..32)
        .step_by(2)
        .map(|i| u8::from_str_radix(&encoded[i..i + 2], 16))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let (stream, _) = DefaultStream::from_unix_stream(UnixStream::connect(format!(
        "/tmp/.X11-unix/X{display}"
    ))?)?;
    let conn = RustConnection::connect_to_stream_with_auth_info(
        stream,
        0,
        b"MIT-MAGIC-COOKIE-1".to_vec(),
        cookie,
    )?;
    Ok(conn)
}
