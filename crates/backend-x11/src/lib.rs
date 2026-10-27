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
    time::Instant,
};
use x11rb::{
    connection::Connection,
    protocol::{composite::ConnectionExt as _, xproto::*, xtest::ConnectionExt as _},
    rust_connection::{DefaultStream, RustConnection},
    wrapper::ConnectionExt as _,
};
mod async_capture;
pub mod clipboard_store;
mod controller_copy;
mod controller_text;
mod damage;
mod inventory;
mod linear_capture;
mod metadata_events;
mod readback;
mod related_windows;
mod render_scale;
mod shared_capture;
mod window_size;
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
    /// Validate a compositor object bound during a trusted X11 scan. Returning
    /// true permits input without asking the busy X server to resolve the XID.
    /// Implementations must reject replacement objects, not rebind on input.
    fn input_target(&self, _window: u32, _focus: bool) -> Result<bool> {
        Ok(false)
    }
    fn logical_layout(&self, _owners: &HashMap<u32, u32>) -> Result<HashMap<u32, LogicalLayout>> {
        Ok(HashMap::new())
    }
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
/// Compositor client coordinates, independent of Xwayland backing pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogicalLayout {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
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
    connection_source: Option<async_capture::ConnectionSource>,
    async_capture: Option<async_capture::AsyncCapture>,
    window_scope: Option<related_windows::Scope>,
    text_transfer: Option<TextTransfer>,
    copy_transfer: Option<CopyTransfer>,
    scale_factor: u8,
    shared_capture: RefCell<shared_capture::SharedCapture>,
    damage: RefCell<damage::Damage>,
    metadata: metadata_events::MetadataEvents,
    pending_event: RefCell<Option<(x11rb::protocol::Event, u64)>>,
    keymap: RefCell<Option<GetKeyboardMappingReply>>,
    render_scale: RefCell<render_scale::RenderScale>,
    visuals: HashMap<u32, u32>,
    desktop_input: Option<Arc<dyn DesktopInput>>,
    desktop_geometry: Option<Box<dyn DesktopGeometry>>,
    size_adjustments: HashMap<u32, window_size::Adjustment>,
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
    pub fn logical_layout(&self) -> Result<HashMap<u32, LogicalLayout>> {
        let Some(geometry) = &self.desktop_geometry else {
            return Ok(HashMap::new());
        };
        let owners = self
            .windows
            .keys()
            .filter_map(|id| {
                let pid = self.client_pid(*id).ok()?;
                self.window_scope
                    .as_ref()
                    .is_some_and(|scope| scope.contains(*id, pid))
                    .then_some((*id, pid))
            })
            .collect();
        geometry.logical_layout(&owners)
    }
    pub fn metadata_events(&self) -> bool {
        self.metadata.enabled()
    }
    pub fn metadata_ready(&self) -> bool {
        self.metadata.pending()
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
        self.invalidate_async_capture(None, false);
    }
    pub fn refresh_window(&self, id: u32) {
        self.damage.borrow_mut().retry(id);
        self.invalidate_async_capture(Some(id), false);
    }
    /// Readiness only; ownership remains with this backend for the controller lease.
    pub fn event_fd(&self) -> std::os::fd::RawFd {
        use std::os::fd::AsRawFd;
        self.conn.stream().as_raw_fd()
    }
    pub fn connect(display: &str) -> Result<Self> {
        let (conn, n) = x11rb::connect(Some(display))?;
        let mut backend = Self::from_connection(conn, n, None)?;
        backend.connection_source = Some(async_capture::ConnectionSource::Display(display.into()));
        Ok(backend)
    }
    pub fn connect_existing(display: u16, authority: &Path, process_id: u32) -> Result<Self> {
        let conn = authenticated_connection(display, authority)?;
        conn.composite_query_version(0, 4)?.reply()?;
        let mut backend = Self::from_connection(conn, 0, Some(process_id))?;
        backend.existing_display = Some(display);
        backend.connection_source = Some(async_capture::ConnectionSource::Existing(
            display,
            authority.into(),
        ));
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
            "_NET_CLIENT_LIST_STACKING",
            "_NET_WM_NAME",
            "_NET_WM_WINDOW_TYPE",
            "_NET_WM_WINDOW_TYPE_TOOLTIP",
            "_NET_WM_WINDOW_TYPE_NOTIFICATION",
            "_NET_WM_WINDOW_TYPE_DND",
            "_NET_WM_WINDOW_TYPE_MENU",
            "_NET_WM_WINDOW_TYPE_DROPDOWN_MENU",
            "_NET_WM_WINDOW_TYPE_POPUP_MENU",
            "_NET_WM_WINDOW_TYPE_COMBO",
            "_NET_WM_WINDOW_TYPE_DIALOG",
            "UTF8_STRING",
            "WM_TRANSIENT_FOR",
            "WM_NORMAL_HINTS",
            "WM_PROTOCOLS",
            "WM_DELETE_WINDOW",
            "_NET_ACTIVE_WINDOW",
            "_NET_WM_STATE",
            "_NET_WM_STATE_MAXIMIZED_HORZ",
            "_NET_WM_STATE_MAXIMIZED_VERT",
            "_NET_MOVERESIZE_WINDOW",
            "_NET_WM_PID",
            "_NET_FRAME_EXTENTS",
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
        let metadata = metadata_events::MetadataEvents::new(&conn, root);
        let render_scale = RefCell::new(render_scale::RenderScale::new(&conn, root));
        let setup = conn.setup();
        let keymap = conn
            .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)?
            .reply()?;
        Ok(Self {
            conn,
            root,
            atoms,
            windows: HashMap::new(),
            pressed: RefCell::new(HashSet::new()),
            process_id,
            root_window: None,
            existing_display: None,
            connection_source: None,
            async_capture: None,
            window_scope: None,
            text_transfer: None,
            copy_transfer: None,
            scale_factor: 1,
            shared_capture,
            damage,
            metadata,
            pending_event: RefCell::new(None),
            keymap: RefCell::new(Some(keymap)),
            render_scale,
            visuals: HashMap::new(),
            desktop_input: None,
            desktop_geometry: None,
            size_adjustments: HashMap::new(),
        })
    }
    pub fn set_scale_factor(&mut self, scale: u8) -> Result<()> {
        paneferry_protocol::validate_scale(scale)?;
        self.scale_factor = scale;
        Ok(())
    }
    /// Publish logical font DPI only on a newly created private X display.
    /// Xft-aware applications read this resource when starting; an existing desktop's
    /// resource database belongs to its user session and must not be changed.
    pub fn set_private_display_dpi(&self, scale: u8) -> Result<()> {
        paneferry_protocol::validate_scale(scale)?;
        if self.existing_display.is_some() {
            bail!("cannot change an existing desktop's DPI")
        }
        let dpi = u16::from(scale) * 96;
        let resource_manager = self
            .conn
            .intern_atom(false, b"RESOURCE_MANAGER")?
            .reply()?
            .atom;
        self.conn
            .change_property8(
                PropMode::REPLACE,
                self.root,
                resource_manager,
                AtomEnum::STRING,
                format!("Xft.dpi:\t{dpi}\n").as_bytes(),
            )?
            .check()?;
        self.conn.flush()?;
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
    fn window_kind(&self, id: u32, override_redirect: bool, parent: bool) -> &'static str {
        let types = self
            .prop(id, "_NET_WM_WINDOW_TYPE", AtomEnum::ATOM)
            .ok()
            .and_then(|p| p.value32().map(|v| v.collect::<Vec<_>>()))
            .unwrap_or_default();
        for (name, kind) in [
            ("TOOLTIP", "tooltip"),
            ("NOTIFICATION", "passive"),
            ("DND", "passive"),
            ("MENU", "menu"),
            ("DROPDOWN_MENU", "menu"),
            ("POPUP_MENU", "menu"),
            ("COMBO", "menu"),
            ("DIALOG", "dialog"),
        ] {
            if types.contains(&self.atom(&format!("_NET_WM_WINDOW_TYPE_{name}"))) {
                return kind;
            }
        }
        if override_redirect {
            "popup"
        } else if parent {
            "dialog"
        } else {
            "window"
        }
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
    fn stack_order(&self, mut id: u32, roots: &HashMap<u32, u32>) -> u32 {
        // QueryTree lists bottom-to-top. Managed clients may be nested inside
        // a WM frame; override-redirect surfaces normally sit directly at root.
        for _ in 0..16 {
            if let Some(order) = roots.get(&id) {
                return *order;
            }
            let Ok(reply) = self.conn.query_tree(id) else {
                break;
            };
            let Ok(tree) = reply.reply() else { break };
            if tree.parent == id || tree.parent == self.root {
                break;
            }
            id = tree.parent;
        }
        0
    }
    pub fn includes_window(&self, id: u32) -> bool {
        self.windows.contains_key(&id) && self.belongs_to_process(id)
    }
    pub fn retry_window_adjustments(&mut self) {
        self.size_adjustments.clear();
    }
    pub fn scan(&mut self) -> Result<Vec<Window>> {
        self.metadata.clear();
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
        self.size_adjustments.retain(|id, _| ids.contains(id));
        let stacking: HashMap<u32, u32> = self
            .conn
            .query_tree(self.root)?
            .reply()?
            .children
            .into_iter()
            .enumerate()
            .map(|(order, id)| (id, order as u32))
            .collect();
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
        let mut size_error = None;
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
                self.size_adjustments.remove(&id);
                continue;
            }
            self.metadata.watch(&self.conn, id, a.your_event_mask);
            self.visuals.insert(id, a.visual);
            let Ok(g) = self.conn.get_geometry(id)?.reply() else {
                continue;
            };
            if paneferry_protocol::dimensions(g.width.into(), g.height.into()).is_err() {
                // The WM enforces minimums, increments and aspect constraints.
                // Repeatedly requesting an impossible height never makes this
                // window discoverable. Keep errors separate from slow startup.
                let adjustment = self.fit_oversized_window(
                    id,
                    (g.width.into(), g.height.into()),
                    a.override_redirect,
                );
                if let Err(error) = adjustment {
                    if self.root_window == Some(id) {
                        return Err(error);
                    }
                    size_error = Some(error);
                }
                continue;
            }
            self.size_adjustments.remove(&id);
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
            let kind = self.window_kind(id, a.override_redirect, parent_id.is_some());
            let overlay = matches!(kind, "tooltip" | "passive" | "menu" | "popup");
            let mut w = Window {
                window_id: id,
                parent_id,
                title,
                role: if overlay {
                    "popup"
                } else if parent_id.is_some() {
                    "dialog"
                } else {
                    "toplevel"
                }
                .into(),
                presentation: Some(paneferry_protocol::WindowPresentation {
                    x: f64::from(pos.dst_x),
                    y: f64::from(pos.dst_y),
                    width: f64::from(g.width),
                    height: f64::from(g.height),
                    kind: kind.into(),
                    order: self.stack_order(id, &stacking),
                }),
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
        if out.is_empty() {
            if let Some(error) = size_error {
                return Err(error);
            }
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
        self.visuals.retain(|id, _| self.windows.contains_key(id));
        self.damage.borrow_mut().sync(&self.conn, &self.windows)?;
        self.sync_async_capture();
        Ok(out)
    }
    fn fit_oversized_window(&mut self, id: u32, current: (u32, u32), popup: bool) -> Result<()> {
        let hints = x11rb::properties::WmSizeHints::get_normal_hints(&self.conn, id)?
            .reply()
            .map_err(|_| anyhow::anyhow!("window_size_unavailable"))?
            .unwrap_or_default();
        let mut bounds = (4096, 4096);
        if self.existing_display.is_none() {
            let screen = self.conn.get_geometry(self.root)?.reply()?;
            let frame: Vec<u32> = self
                .prop(id, "_NET_FRAME_EXTENTS", AtomEnum::CARDINAL)
                .ok()
                .and_then(|reply| reply.value32().map(|values| values.take(4).collect()))
                .unwrap_or_default();
            let (horizontal, vertical) = if frame.len() == 4 {
                (
                    frame[0].saturating_add(frame[1]),
                    frame[2].saturating_add(frame[3]),
                )
            } else {
                (0, 0)
            };
            bounds = (
                u32::from(screen.width).saturating_sub(horizontal),
                u32::from(screen.height).saturating_sub(vertical),
            );
        }
        let target =
            window_size::fit(current, bounds, &hints).context("window_size_unavailable")?;
        let now = Instant::now();
        let send = match self.size_adjustments.get_mut(&id) {
            Some(request) => request.update(target, now).map_err(anyhow::Error::msg)?,
            None => {
                self.size_adjustments
                    .insert(id, window_size::Adjustment::new(target, now));
                true
            }
        };
        if send {
            self.resize_window(id, target.0, target.1, popup)?;
        }
        Ok(())
    }
    /// Peek without taking an event away from clipboard/selection handlers.
    /// A synchronous reply or a bounded drain can leave events in x11rb even
    /// when the socket is no longer readable.
    pub fn events_pending(&self) -> Result<bool> {
        let mut pending = self.pending_event.borrow_mut();
        if pending.is_none() {
            *pending = self.conn.poll_for_event_with_sequence()?;
        }
        Ok(pending.is_some())
    }
    fn next_event(&self) -> Result<Option<x11rb::protocol::Event>> {
        for _ in 0..256 {
            let queued = self.pending_event.borrow_mut().take();
            let event = match queued {
                Some(event) => Some(event),
                None => self.conn.poll_for_event_with_sequence()?,
            };
            if let Some((event, _)) = &event {
                if matches!(event, x11rb::protocol::Event::MappingNotify(_)) {
                    self.keymap.borrow_mut().take();
                }
                self.metadata
                    .notify(event, self.root, &self.windows, &self.atoms);
                use x11rb::protocol::Event;
                let changed = match event {
                    Event::DestroyNotify(e) => Some(e.window),
                    Event::UnmapNotify(e) => Some(e.window),
                    Event::MapNotify(e) => Some(e.window),
                    Event::ReparentNotify(e) => Some(e.window),
                    Event::ConfigureNotify(e)
                        if self.windows.get(&e.window).is_some_and(|w| {
                            (w.width, w.height) != (u32::from(e.width), u32::from(e.height))
                        }) =>
                    {
                        Some(e.window)
                    }
                    _ => None,
                };
                if let Some(id) = changed {
                    self.invalidate_async_capture(Some(id), true);
                }
            }
            match event {
                Some((x11rb::protocol::Event::DamageNotify(event), sequence)) => {
                    self.damage.borrow_mut().notify(&event, sequence)
                }
                event => return Ok(event.map(|(event, _)| event)),
            }
        }
        Ok(None)
    }
    pub fn pump_capture_events(&self) -> Result<()> {
        // A transfer's event handler must see its own selection/property events.
        if self.text_transfer.is_some() || self.copy_transfer.is_some() {
            return self.poll_async_capture();
        }
        for _ in 0..256 {
            if self.next_event()?.is_none() {
                break;
            }
        }
        self.poll_async_capture()
    }
    pub fn capture_pending(&self) -> bool {
        self.async_capture
            .as_ref()
            .map_or_else(|| self.damage.borrow().pending(), |c| c.pending())
    }
    pub fn capture_needed(&self, window: u32) -> bool {
        self.async_capture
            .as_ref()
            .map_or_else(|| self.damage.borrow().needed(window), |c| c.needed(window))
    }
    pub fn capture(&self, w: &Window) -> Result<CapturedImage> {
        self.capture_scaled(w, w.width, w.height)
    }
    pub fn capture_scaled(&self, w: &Window, width: u32, height: u32) -> Result<CapturedImage> {
        paneferry_protocol::dimensions(width, height)?;
        let result = self.capture_image(w, (width, height));
        if result.is_err() {
            self.damage.borrow_mut().retry(w.window_id);
        }
        result
    }
    fn capture_image(&self, w: &Window, wanted: (u32, u32)) -> Result<CapturedImage> {
        self.known(w.window_id)?;
        self.damage
            .borrow_mut()
            .acknowledge(&self.conn, w.window_id)?;
        readback::capture(
            &self.conn,
            &mut self.shared_capture.borrow_mut(),
            &mut self.render_scale.borrow_mut(),
            &readback::Surface {
                window: w.window_id,
                source: (w.width, w.height),
                target: wanted,
                visual: self.visuals.get(&w.window_id).copied(),
                composite: self.process_id.is_some(),
            },
        )
    }

    fn known(&self, id: u32) -> Result<()> {
        if !self.windows.contains_key(&id) || !self.belongs_to_process(id) {
            bail!("unknown window")
        };
        Ok(())
    }
    fn compositor_input(&self, id: u32, focus: bool) -> Result<bool> {
        let Some(geometry) = self
            .desktop_geometry
            .as_ref()
            .filter(|_| self.desktop_input.is_some())
        else {
            return Ok(false);
        };
        let Some(scope) = &self.window_scope else {
            return Ok(false);
        };
        anyhow::ensure!(
            self.windows.contains_key(&id) && scope.current(id),
            "unknown window"
        );
        if focus && !self.windows[&id].visible {
            bail!("window is hidden")
        }
        geometry.input_target(id, focus)
    }
    pub fn configure(&self, id: u32, w: u32, h: u32) -> Result<()> {
        self.known(id)?;
        paneferry_protocol::dimensions(w, h)?;
        self.resize_window(id, w, h, self.windows[&id].role == "popup")
    }
    fn resize_window(&self, id: u32, w: u32, h: u32, popup: bool) -> Result<()> {
        // Recovery runs before an oversized window enters `windows`. It needs
        // the same scoped compositor route as an explicit controller resize.
        if let Some(geometry) = &self.desktop_geometry {
            // After a live Xwayland scale change, Mutter can retain size hints
            // converted using the previous scale. Ask it to re-read the existing
            // property before resizing. Do not replace/remove the application's
            // hints (or change the desktop's DPI). This runs only on explicit
            // resize/recovery, never on the input or per-frame path.
            self.conn
                .send_event(
                    false,
                    id,
                    EventMask::PROPERTY_CHANGE,
                    PropertyNotifyEvent {
                        response_type: PROPERTY_NOTIFY_EVENT,
                        sequence: 0,
                        window: id,
                        atom: AtomEnum::WM_NORMAL_HINTS.into(),
                        time: x11rb::CURRENT_TIME,
                        state: Property::NEW_VALUE,
                    },
                )?
                .check()?;
            if geometry.resize(id, w, h)? {
                return Ok(());
            }
        }
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
        if self.windows[&id].role == "popup" {
            return Ok(());
        }
        if !self.windows[&id].visible {
            if self.windows[&id].role == "popup" {
                bail!("window is hidden")
            }
            // An explicit focus request is also the controller's restore action.
            // Let the WM remap the managed window before attempting input focus.
            self.conn.map_window(id)?.check()?;
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
            self.conn.flush()?;
            return Ok(());
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
        if self.windows[&id].role == "popup" {
            return Ok(());
        }
        let mut focused = self.conn.get_input_focus()?.reply()?.focus;
        // A focused transient popup retains its toolkit keyboard grab.
        let mut popup = focused;
        for _ in 0..16 {
            let Some(window) = self.windows.get(&popup) else {
                break;
            };
            if window.role != "popup" {
                break;
            }
            let Some(parent) = window.parent_id else {
                break;
            };
            if parent == id {
                return Ok(());
            }
            popup = parent;
        }
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
        let w = self.windows.get(&id).context("unknown window")?;
        if x < 0 || y < 0 || x >= w.width as i32 || y >= w.height as i32 || button > 7 {
            self.known(id)?;
            if (1..=7).contains(&button) && !pressed {
                self.release_all();
                return Ok(());
            }
            bail!("invalid pointer")
        };
        if pressed || button == 0 {
            self.require_input()?;
        }
        let compositor = self.compositor_input(id, button != 0 && pressed)?;
        if !compositor {
            self.known(id)?;
        }
        if button != 0 && pressed && !compositor {
            self.focus_for_input(id)?;
        }
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
            let point = self
                .conn
                .translate_coordinates(id, self.root, x as i16, y as i16)?
                .reply()?;
            return input.pointer(point.dst_x.into(), point.dst_y.into(), button, pressed);
        }
        let point = self
            .conn
            .translate_coordinates(id, self.root, x as i16, y as i16)?
            .reply()?;
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
        let mut cached = self.keymap.borrow_mut();
        if cached.is_none() {
            *cached = Some(
                self.conn
                    .get_keyboard_mapping(min, max - min + 1)?
                    .reply()?,
            );
        }
        let map = cached.as_ref().unwrap();
        let code = map
            .keysyms
            .chunks(map.keysyms_per_keycode as usize)
            .position(|syms| syms.contains(&keysym))
            .map(|i| min + i as u8)
            .ok_or_else(|| anyhow::anyhow!("keysym unavailable in server layout"))?;
        Ok(code)
    }
    pub fn key(&self, id: u32, keysym: u32, pressed: bool) -> Result<()> {
        if pressed {
            self.require_input()?;
        }
        let compositor = self.compositor_input(id, pressed)?;
        if !compositor {
            self.known(id)?;
        }
        if pressed && !compositor {
            self.focus_for_input(id)?;
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

#[cfg(test)]
mod geometry_tests {
    use super::*;
    use std::sync::Mutex;

    struct Geometry {
        conn: RustConnection,
        calls: Arc<Mutex<Vec<(u32, u32, u32)>>>,
        handled: bool,
        fail: bool,
    }
    impl DesktopGeometry for Geometry {
        fn resize(&self, id: u32, w: u32, h: u32) -> Result<bool> {
            self.calls.lock().unwrap().push((id, w, h));
            if self.fail {
                bail!("window_unavailable");
            }
            if self.handled {
                self.conn
                    .configure_window(id, &ConfigureWindowAux::new().width(w).height(h))?
                    .check()?;
            }
            Ok(self.handled)
        }
        fn pointer(&self, _: u32, _: i32, _: i32, _: u32, _: u32) -> Result<Option<(i32, i32)>> {
            Ok(None)
        }
    }

    #[test]
    #[ignore = "requires a private Xvfb DISPLAY; run with xvfb-run"]
    fn oversized_discovery_and_controller_resize_share_scoped_geometry() -> Result<()> {
        let (conn, screen) = x11rb::connect(None)?;
        let mut backend = Backend::from_connection(conn, screen, None)?;
        backend.existing_display = Some(0);
        let id = backend.conn.generate_id()?;
        backend
            .conn
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                id,
                backend.root,
                0,
                0,
                4100,
                1000,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new(),
            )?
            .check()?;
        backend.conn.map_window(id)?.check()?;
        backend
            .conn
            .change_property32(
                PropMode::REPLACE,
                backend.root,
                backend.atom("_NET_CLIENT_LIST"),
                AtomEnum::WINDOW,
                &[id],
            )?
            .check()?;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let geometry = |handled, fail| -> Result<Box<dyn DesktopGeometry>> {
            Ok(Box::new(Geometry {
                conn: x11rb::connect(None)?.0,
                calls: calls.clone(),
                handled,
                fail,
            }))
        };
        backend.set_desktop_geometry(geometry(true, false)?);
        assert!(!backend.windows.contains_key(&id));
        backend.scan()?;
        let resized = backend.conn.get_geometry(id)?.reply()?;
        paneferry_protocol::dimensions(resized.width.into(), resized.height.into())?;
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "initial discovery must use compositor geometry"
        );
        backend.scan()?;
        assert!(backend.windows.contains_key(&id));
        backend.configure(id, 900, 600)?;
        assert_eq!(calls.lock().unwrap().last(), Some(&(id, 900, 600)));
        assert_eq!(backend.conn.get_geometry(id)?.reply()?.width, 900);

        // A rejected scope must never fall through to direct X11 mutation.
        backend.set_desktop_geometry(geometry(false, true)?);
        assert!(backend.resize_window(id, 700, 500, true).is_err());
        assert_eq!(backend.conn.get_geometry(id)?.reply()?.width, 900);
        // A compositor that does not handle this window retains X11 fallback.
        backend.set_desktop_geometry(geometry(false, false)?);
        backend.resize_window(id, 700, 500, true)?;
        assert_eq!(backend.conn.get_geometry(id)?.reply()?.width, 700);
        backend.conn.destroy_window(id)?.check()?;
        Ok(())
    }
}
