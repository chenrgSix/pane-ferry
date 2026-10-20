use anyhow::{bail, Context, Result};
use remoteapp_protocol::Window;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    os::unix::net::UnixStream,
    path::Path,
    process::{Command, Stdio},
};
use x11rb::{
    connection::Connection,
    protocol::{composite::ConnectionExt as _, xproto::*, xtest::ConnectionExt as _},
    rust_connection::{DefaultStream, RustConnection},
};
mod controller_copy;
mod controller_text;
use controller_copy::CopyTransfer;
use controller_text::TextTransfer;
pub struct Backend {
    conn: RustConnection,
    root: u32,
    atoms: HashMap<String, u32>,
    pub windows: HashMap<u32, Window>,
    pressed: RefCell<HashSet<(u8, bool)>>,
    process_id: Option<u32>,
    text_transfer: Option<TextTransfer>,
    copy_transfer: Option<CopyTransfer>,
}
impl Backend {
    pub fn connect(display: &str) -> Result<Self> {
        let (conn, n) = x11rb::connect(Some(display))?;
        Self::from_connection(conn, n, None)
    }
    pub fn connect_existing(display: u16, authority: &Path, process_id: u32) -> Result<Self> {
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
        conn.composite_query_version(0, 4)?.reply()?;
        Self::from_connection(conn, 0, Some(process_id))
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
            "_NET_WM_PID",
            "_NET_SUPPORTING_WM_CHECK",
            "WM_CLASS",
            "CLIPBOARD",
            "TARGETS",
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
        Ok(Self {
            conn,
            root,
            atoms,
            windows: HashMap::new(),
            pressed: RefCell::new(HashSet::new()),
            process_id,
            text_transfer: None,
            copy_transfer: None,
        })
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
        let Some(process_id) = self.process_id else {
            return true;
        };
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
    pub fn scan(&mut self) -> Result<Vec<Window>> {
        let clients = self
            .prop(self.root, "_NET_CLIENT_LIST", AtomEnum::WINDOW)?
            .value32()
            .map(|x| x.collect::<Vec<_>>())
            .unwrap_or_default();
        let mut ids = clients;
        // Unmapping/withdrawing a window is not destruction. Keep known XIDs
        // until the server confirms they no longer exist, including cached menus.
        ids.extend(self.windows.keys().copied());
        for id in self.conn.query_tree(self.root)?.reply()?.children {
            if let Ok(a) = self.conn.get_window_attributes(id)?.reply() {
                if a.override_redirect {
                    ids.push(id)
                }
            }
        }
        ids.sort_unstable();
        ids.dedup();
        let mut out = Vec::new();
        for id in ids {
            if !self.belongs_to_process(id) {
                continue;
            }
            if out.len() >= 16 {
                break;
            }
            let Ok(a) = self.conn.get_window_attributes(id)?.reply() else {
                continue;
            };
            let Ok(g) = self.conn.get_geometry(id)?.reply() else {
                continue;
            };
            if remoteapp_protocol::dimensions(g.width.into(), g.height.into()).is_err() {
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
                .prop(id, "WM_TRANSIENT_FOR", AtomEnum::WINDOW)
                .ok()
                .and_then(|p| p.value32().and_then(|mut v| v.next()))
                .filter(|id| self.belongs_to_process(*id));
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
        Ok(out)
    }
    pub fn capture(&self, w: &Window) -> Result<Vec<u8>> {
        self.known(w.window_id)?;
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
        let image = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                pixmap.unwrap_or(w.window_id),
                0,
                0,
                w.width as u16,
                w.height as u16,
                u32::MAX,
            )?
            .reply();
        if let Some(pixmap) = pixmap {
            self.conn.free_pixmap(pixmap)?;
        }
        let img = image?;
        let visual_id = if img.visual == 0 {
            self.conn
                .get_window_attributes(w.window_id)?
                .reply()?
                .visual
        } else {
            img.visual
        };
        let setup = self.conn.setup();
        let f = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == img.depth)
            .ok_or_else(|| anyhow::anyhow!("missing pixel format"))?;
        let visual = setup
            .roots
            .iter()
            .flat_map(|s| &s.allowed_depths)
            .flat_map(|d| &d.visuals)
            .find(|v| v.visual_id == visual_id)
            .ok_or_else(|| anyhow::anyhow!("missing visual"))?;
        remoteapp_capture::encode(
            w.width,
            w.height,
            &img.data,
            f.bits_per_pixel,
            f.scanline_pad,
            setup.image_byte_order == ImageOrder::LSB_FIRST,
            (visual.red_mask, visual.green_mask, visual.blue_mask),
        )
    }
    fn known(&self, id: u32) -> Result<()> {
        if !self.windows.contains_key(&id) || !self.belongs_to_process(id) {
            bail!("unknown window")
        };
        Ok(())
    }
    pub fn configure(&self, id: u32, w: u32, h: u32) -> Result<()> {
        self.known(id)?;
        remoteapp_protocol::dimensions(w, h)?;
        self.conn
            .configure_window(id, &ConfigureWindowAux::new().width(w).height(h))?
            .check()?;
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
            bail!("invalid pointer")
        };
        self.focus(id)?;
        self.conn
            .warp_pointer(x11rb::NONE, id, 0, 0, 0, 0, x as i16, y as i16)?
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
    pub fn key(&self, id: u32, keysym: u32, pressed: bool) -> Result<()> {
        self.known(id)?;
        if pressed {
            self.focus(id)?;
        }
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
