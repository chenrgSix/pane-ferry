use anyhow::{bail, Result};
use remoteapp_protocol::Window;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
};
use x11rb::{
    connection::Connection,
    protocol::{xproto::*, xtest::ConnectionExt as _},
    rust_connection::RustConnection,
};
pub struct Backend {
    conn: RustConnection,
    root: u32,
    atoms: HashMap<String, u32>,
    pub windows: HashMap<u32, Window>,
    pressed: RefCell<HashSet<(u8, bool)>>,
}
impl Backend {
    pub fn connect(display: &str) -> Result<Self> {
        let (conn, n) = x11rb::connect(Some(display))?;
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
    pub fn scan(&mut self) -> Result<Vec<Window>> {
        let clients = self
            .prop(self.root, "_NET_CLIENT_LIST", AtomEnum::WINDOW)?
            .value32()
            .map(|x| x.collect::<Vec<_>>())
            .unwrap_or_default();
        let mut ids = clients; // Also capture override-redirect popups, without exposing the root desktop.
        for id in self.conn.query_tree(self.root)?.reply()?.children {
            if let Ok(a) = self.conn.get_window_attributes(id)?.reply() {
                if a.override_redirect && a.map_state == MapState::VIEWABLE {
                    ids.push(id)
                }
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids.truncate(16);
        let mut out = Vec::new();
        for id in ids {
            let Ok(a) = self.conn.get_window_attributes(id)?.reply() else {
                continue;
            };
            if a.map_state != MapState::VIEWABLE {
                continue;
            }
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
                .and_then(|p| p.value32().and_then(|mut v| v.next()));
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
            };
            if let Some(old) = self.windows.get(&id) {
                w.geometry_revision = old.geometry_revision
                    + u64::from(old.width != w.width || old.height != w.height);
            }
            out.push(w);
        }
        self.windows = out.iter().cloned().map(|w| (w.window_id, w)).collect();
        Ok(out)
    }
    pub fn capture(&self, w: &Window) -> Result<Vec<u8>> {
        let img = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                w.window_id,
                0,
                0,
                w.width as u16,
                w.height as u16,
                u32::MAX,
            )?
            .reply()?;
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
            .find(|v| v.visual_id == img.visual)
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
        if !self.windows.contains_key(&id) {
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
        self.focus(id)?;
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
