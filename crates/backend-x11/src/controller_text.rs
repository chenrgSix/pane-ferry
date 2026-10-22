//! Controller text and paste delivery. No background clipboard sync.
//! Selection delivery follows https://www.x.org/releases/current/doc/xorg-docs/icccm/icccm.html.
use super::*;
use std::time::{Duration, Instant};
use x11rb::{
    protocol::{
        res::{ClientIdMask, ClientIdSpec, ConnectionExt as _},
        Event,
    },
    wrapper::ConnectionExt as _,
};

pub(super) struct TextTransfer {
    owner: u32,
    window_id: u32,
    terminal: bool,
    key_interval: Duration,
    paste_at: Instant,
    pasted_at: Option<Instant>,
    completed: bool,
    target_pid: u32,
    bridge_pid: Option<u32>,
    offers: HashMap<u32, Vec<u8>>,
    incremental: HashMap<(u32, u32), (u32, usize)>,
    timestamp: u32,
    requestor: Option<(u32, u32)>,
    data_consumed: bool,
    deadline: Instant,
}
impl Backend {
    pub(super) fn clipboard_shortcut(
        &self,
        window: u32,
        letter: u8,
        terminal: bool,
        interval: Duration,
    ) -> Result<()> {
        let mut symbols = vec![0xffe3];
        if terminal {
            symbols.push(0xffe1);
        }
        symbols.push(u32::from(letter));
        // Firefox also needs distinct shortcut event timestamps. Its GTK key
        // handling can merge rapid repeated paste shortcuts. Other native X11
        // applications keep the zero-delay path.
        for symbol in &symbols {
            self.key(window, *symbol, true)?;
            if !interval.is_zero() {
                std::thread::sleep(interval);
            }
        }
        for symbol in symbols.into_iter().rev() {
            self.key(window, symbol, false)?;
            if !interval.is_zero() {
                std::thread::sleep(interval);
            }
        }
        Ok(())
    }
    pub(super) fn clipboard_key_interval(class: &str, bridged: bool) -> Duration {
        Duration::from_millis(if bridged {
            12
        } else if class.contains("firefox") || class.contains("navigator") {
            4
        } else {
            0
        })
    }
    pub(super) fn clipboard_bridge_pid(&self) -> Option<u32> {
        use std::os::unix::fs::MetadataExt;
        let wm = self
            .prop(self.root, "_NET_SUPPORTING_WM_CHECK", AtomEnum::WINDOW)
            .ok()?
            .value32()?
            .next()?;
        let mut pid = self.client_pid(wm).ok()?;
        let uid = std::fs::metadata("/proc/self").ok()?.uid();
        for _ in 0..4 {
            let root = std::path::PathBuf::from(format!("/proc/{pid}"));
            if std::fs::metadata(&root).ok()?.uid() != uid {
                return None;
            }
            let exe = std::fs::read_link(root.join("exe")).ok()?;
            if [
                "/usr/bin/gnome-shell",
                "/usr/bin/kwin_wayland",
                "/usr/bin/weston",
            ]
            .iter()
            .any(|path| exe == std::path::Path::new(path))
            {
                return Some(pid);
            }
            let stat = std::fs::read_to_string(root.join("stat")).ok()?;
            pid = stat
                .rsplit_once(')')?
                .1
                .split_whitespace()
                .nth(1)?
                .parse()
                .ok()?;
        }
        None
    }
    pub(super) fn application_pid(&self, window: u32) -> Result<u32> {
        // Electron can create the visible window from its GPU process while
        // clipboard helpers belong to the main process named by _NET_WM_PID.
        if let Some(scope) = &self.window_scope {
            self.known(window)?;
            return self
                .prop(window, "_NET_WM_PID", AtomEnum::CARDINAL)
                .ok()
                .and_then(|p| p.value32().and_then(|mut v| v.next()))
                .and_then(|pid| scope.clipboard_pid(window, pid))
                .map(Ok)
                .unwrap_or_else(|| self.client_pid(window));
        }
        if let Some(pid) = self.process_id {
            return Ok(pid);
        }
        self.prop(window, "_NET_WM_PID", AtomEnum::CARDINAL)
            .ok()
            .and_then(|p| p.value32().and_then(|mut values| values.next()))
            .map(Ok)
            .unwrap_or_else(|| self.client_pid(window))
    }
    pub(super) fn client_pid(&self, window: u32) -> Result<u32> {
        let reply = self
            .conn
            .res_query_client_ids(&[ClientIdSpec {
                client: window,
                mask: ClientIdMask::LOCAL_CLIENT_PID,
            }])?
            .reply()?;
        reply
            .ids
            .iter()
            .find_map(|id| id.value.first().copied())
            .context("X11 client PID unavailable")
    }
    pub fn text_pending(&self) -> bool {
        self.text_transfer
            .as_ref()
            .is_some_and(|transfer| !transfer.completed)
            || self.copy_transfer.is_some()
    }
    pub fn cancel_text(&mut self) {
        self.cancel_copy();
        if let Some(transfer) = self.text_transfer.take() {
            let _ = self.conn.destroy_window(transfer.owner);
            let _ = self.conn.flush();
        }
    }
    pub fn commit_text(&mut self, window_id: u32, text: String) -> Result<()> {
        paneferry_protocol::validate_text(&text)?;
        let offers = [
            "UTF8_STRING",
            "TEXT",
            "text/plain;charset=utf-8",
            "text/plain",
        ]
        .into_iter()
        .map(|name| (name.to_string(), text.as_bytes().to_vec()))
        .collect();
        self.paste_payload(window_id, offers, false)
    }
    pub fn paste_bundle(&mut self, window_id: u32, transfer_id: &str) -> Result<()> {
        let offers = crate::clipboard_store::ClipboardStore::open()?.offers(transfer_id)?;
        self.paste_payload(window_id, offers, true)
    }
    fn paste_payload(
        &mut self,
        window_id: u32,
        offers: Vec<(String, Vec<u8>)>,
        rich: bool,
    ) -> Result<()> {
        self.known(window_id)?;
        self.require_input()?;
        if self.text_pending() {
            bail!("controller text transfer already pending")
        }
        self.cancel_text();
        // XRes also identifies hidden toolkit requestor windows, without trusting
        // arbitrary requestors or serving a desktop clipboard manager.
        self.conn.res_query_version(1, 2)?.reply()?;
        let target_pid = self.application_pid(window_id)?;
        let bridge_pid = self.clipboard_bridge_pid();
        self.release_all();
        // Preserve Firefox/GTK's focused child. Re-focusing its toplevel can
        // cancel the active editor (including an in-progress address edit).
        self.focus_for_input(window_id)?;
        let owner = self.conn.generate_id()?;
        self.conn
            .create_window(
                0,
                owner,
                self.root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )?
            .check()?;
        let result = (|| {
            // Get a real server timestamp without changing the global environment.
            self.conn.change_property8(
                PropMode::REPLACE,
                owner,
                self.atom("_LAN_REMOTEAPP_TEXT_TIME"),
                AtomEnum::STRING,
                b"1",
            )?;
            self.conn.get_input_focus()?.reply()?;
            let mut timestamp = None;
            for _ in 0..256 {
                match self.next_event()? {
                    Some(Event::PropertyNotify(e)) if e.window == owner => {
                        timestamp = Some(e.time);
                        break;
                    }
                    Some(_) => {}
                    None => break,
                }
            }
            let timestamp = timestamp.context("X11 selection timestamp unavailable")?;
            self.conn
                .set_selection_owner(owner, self.atom("CLIPBOARD"), timestamp)?
                .check()?;
            if self
                .conn
                .get_selection_owner(self.atom("CLIPBOARD"))?
                .reply()?
                .owner
                != owner
            {
                bail!("cannot own controller text selection")
            }
            // Terminals reserve Ctrl+V for a literal character; their paste is Ctrl+Shift+V.
            let class = self.prop(window_id, "WM_CLASS", AtomEnum::STRING)?.value;
            let class = String::from_utf8_lossy(&class).to_ascii_lowercase();
            let terminal = [
                "xterm",
                "terminal",
                "konsole",
                "kitty",
                "alacritty",
                "tilix",
            ]
            .iter()
            .any(|name| class.contains(name));
            self.text_transfer = Some(TextTransfer {
                owner,
                window_id,
                terminal,
                key_interval: Self::clipboard_key_interval(&class, bridge_pid.is_some()),
                // Firefox snapshots clipboard offers asynchronously. If Ctrl+V
                // overtakes the new offer, it can paste its previous snapshot.
                paste_at: Instant::now()
                    + Duration::from_millis(if bridge_pid.is_some() {
                        100
                    } else if class.contains("firefox") || class.contains("navigator") {
                        16
                    } else {
                        0
                    }),
                pasted_at: None,
                completed: false,
                target_pid,
                bridge_pid,
                offers: offers
                    .into_iter()
                    .map(|(name, data)| (self.atom(&name), data))
                    .collect(),
                incremental: HashMap::new(),
                timestamp,
                requestor: None,
                data_consumed: false,
                deadline: Instant::now() + Duration::from_secs(if rich { 30 } else { 3 }),
            });
            self.conn.flush()?;
            Ok(())
        })();
        if result.is_err() {
            self.release_all();
            self.cancel_text();
            let _ = self.conn.destroy_window(owner);
        }
        result
    }
    pub fn pump_text(&mut self) -> Result<()> {
        let Some(mut transfer) = self.text_transfer.take() else {
            return Ok(());
        };
        let mut complete = false;
        let mut selection_lost = false;
        let result = (|| {
            for _ in 0..256 {
                let Some(event) = self.next_event()? else {
                    break;
                };
                match event {
                    Event::SelectionRequest(e) if e.owner == transfer.owner => {
                        let property = if e.property == 0 {
                            e.target
                        } else {
                            e.property
                        };
                        let requestor_pid = self.client_pid(e.requestor).ok();
                        let authorized = e.selection == self.atom("CLIPBOARD")
                            && (requestor_pid == Some(transfer.target_pid)
                                || transfer.bridge_pid.is_some()
                                    && requestor_pid == transfer.bridge_pid);
                        let mut accepted = false;
                        if authorized && e.target == self.atom("TARGETS") {
                            if requestor_pid == transfer.bridge_pid && transfer.bridge_pid.is_some()
                            {
                                transfer.paste_at = transfer
                                    .paste_at
                                    .min(Instant::now() + Duration::from_millis(30));
                            }
                            self.conn
                                .change_property32(
                                    PropMode::REPLACE,
                                    e.requestor,
                                    property,
                                    AtomEnum::ATOM,
                                    &std::iter::once(self.atom("TARGETS"))
                                        .chain(std::iter::once(self.atom("TIMESTAMP")))
                                        .chain(transfer.offers.keys().copied())
                                        .collect::<Vec<_>>(),
                                )?
                                .check()?;
                            accepted = true;
                        } else if authorized && e.target == self.atom("TIMESTAMP") {
                            self.conn
                                .change_property32(
                                    PropMode::REPLACE,
                                    e.requestor,
                                    property,
                                    AtomEnum::INTEGER,
                                    &[transfer.timestamp],
                                )?
                                .check()?;
                            accepted = true;
                        } else if authorized && transfer.offers.contains_key(&e.target) {
                            self.conn
                                .change_window_attributes(
                                    e.requestor,
                                    &ChangeWindowAttributesAux::new()
                                        .event_mask(EventMask::PROPERTY_CHANGE),
                                )?
                                .check()?;
                            let bytes = &transfer.offers[&e.target];
                            if bytes.len() > 32 * 1024 {
                                self.conn
                                    .change_property32(
                                        PropMode::REPLACE,
                                        e.requestor,
                                        property,
                                        self.atom("INCR"),
                                        &[bytes.len() as u32],
                                    )?
                                    .check()?;
                                transfer
                                    .incremental
                                    .insert((e.requestor, property), (e.target, 0));
                            } else {
                                self.conn
                                    .change_property8(
                                        PropMode::REPLACE,
                                        e.requestor,
                                        property,
                                        if e.target == self.atom("TEXT") {
                                            self.atom("UTF8_STRING")
                                        } else {
                                            e.target
                                        },
                                        bytes,
                                    )?
                                    .check()?;
                            }
                            transfer.requestor = Some((e.requestor, property));
                            accepted = true;
                        }
                        self.conn
                            .send_event(
                                false,
                                e.requestor,
                                EventMask::NO_EVENT,
                                SelectionNotifyEvent {
                                    response_type: SELECTION_NOTIFY_EVENT,
                                    sequence: 0,
                                    time: e.time,
                                    requestor: e.requestor,
                                    selection: e.selection,
                                    target: e.target,
                                    property: if accepted { property } else { 0 },
                                },
                            )?
                            .check()?;
                    }
                    Event::PropertyNotify(e) if e.state == Property::DELETE => {
                        let key = (e.window, e.atom);
                        if let Some((target, offset)) = transfer.incremental.get_mut(&key) {
                            let bytes = &transfer.offers[target];
                            if *offset > bytes.len() {
                                transfer.data_consumed = true;
                                transfer.incremental.remove(&key);
                            } else {
                                let end = (*offset + 32 * 1024).min(bytes.len());
                                self.conn
                                    .change_property8(
                                        PropMode::REPLACE,
                                        e.window,
                                        e.atom,
                                        *target,
                                        &bytes[*offset..end],
                                    )?
                                    .check()?;
                                *offset = if end == *offset { bytes.len() + 1 } else { end };
                            }
                        } else if transfer.requestor == Some(key) {
                            transfer.data_consumed = true;
                        }
                    }
                    Event::SelectionClear(e) if e.owner == transfer.owner => {
                        if transfer.completed {
                            selection_lost = true;
                            break;
                        }
                        bail!("controller text selection ownership changed")
                    }
                    _ => {}
                }
            }
            if transfer.pasted_at.is_none() && Instant::now() >= transfer.paste_at {
                // The Wayland bridge must first advertise the new selection.
                // Pasting before it receives TARGETS makes Electron read the old offer.
                let id = transfer.window_id;
                self.clipboard_shortcut(id, b'v', transfer.terminal, transfer.key_interval)?;
                transfer.pasted_at = Some(Instant::now());
            }
            if transfer.bridge_pid.is_some()
                && transfer.requestor.is_some()
                // A large image can still be in INCR when the bridge's
                // advertisement delay expires. Keep later input behind it.
                && transfer.incremental.is_empty()
                && transfer
                    .pasted_at
                    .is_some_and(|at| at.elapsed() >= Duration::from_millis(100))
            {
                complete = true;
            }
            if transfer.bridge_pid.is_none()
                && transfer.data_consumed
                && transfer.pasted_at.is_some()
            {
                complete = true;
            }
            if !complete && !transfer.completed && Instant::now() >= transfer.deadline {
                bail!(
                    "application did not accept controller text (data request received: {})",
                    transfer.requestor.is_some()
                )
            }
            self.conn.flush()?;
            Ok(())
        })();
        if selection_lost || result.is_err() {
            let _ = self.conn.destroy_window(transfer.owner);
            let _ = self.conn.flush();
        } else {
            // Keep the selection source alive after delivery. A Wayland bridge
            // advertises an asynchronous offer and may request its bytes later.
            // New input is unblocked, but the next operation/detach replaces it.
            transfer.completed |= complete;
            self.text_transfer = Some(transfer);
        }
        result
    }
}
