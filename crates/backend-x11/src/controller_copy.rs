//! Explicit selection reads, including ICCCM INCR, scoped to the controlled app.
use super::*;
use clipboard_store::{validate_png, ClipboardStore};
use remoteapp_protocol::clipboard::MAX_IMAGE_BYTES;
use std::time::{Duration, Instant};
use x11rb::protocol::{res::ConnectionExt as _, Event};

pub(super) struct CopyTransfer {
    owner: u32,
    window_id: u32,
    request_id: String,
    target_pid: u32,
    bridge_pid: Option<u32>,
    requested: bool,
    cut: bool,
    cut_sent: bool,
    key_interval: Duration,
    target: Option<String>,
    incremental: bool,
    bytes: Vec<u8>,
    deadline: Instant,
    owner_deadline: Instant,
}
pub struct CopiedText {
    pub window_id: u32,
    pub request_id: String,
    pub text: String,
    pub bundle: Option<String>,
}
impl Backend {
    pub(super) fn cancel_copy(&mut self) {
        if let Some(t) = self.copy_transfer.take() {
            let _ = self.conn.destroy_window(t.owner);
            let _ = self.conn.flush();
        }
    }
    pub fn copy_text(&mut self, window: u32, id: String) -> Result<()> {
        self.copy_or_cut(window, id, false, false)
    }
    pub fn cut_text(&mut self, window: u32, id: String) -> Result<()> {
        self.copy_or_cut(window, id, true, false)
    }
    pub fn copy_bundle(&mut self, window: u32, id: String, cut: bool) -> Result<()> {
        self.copy_or_cut(window, id, cut, true)
    }
    pub fn cut_pending(&self) -> bool {
        self.copy_transfer.as_ref().is_some_and(|t| t.cut)
    }
    fn prepare_copy_owner(&self, owner: u32) -> Result<()> {
        self.conn
            .set_selection_owner(owner, self.atom("CLIPBOARD"), x11rb::CURRENT_TIME)?
            .check()?;
        if self
            .conn
            .get_selection_owner(self.atom("CLIPBOARD"))?
            .reply()?
            .owner
            != owner
        {
            bail!("cannot prepare explicit copy")
        }
        Ok(())
    }
    fn copy_or_cut(&mut self, window: u32, id: String, cut: bool, rich: bool) -> Result<()> {
        self.known(window)?;
        self.require_input()?;
        if self.text_pending() {
            bail!("clipboard operation already pending")
        }
        let class = self.prop(window, "WM_CLASS", AtomEnum::STRING)?.value;
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
        .any(|s| class.contains(s));
        if cut && terminal {
            bail!("terminal clipboard cut unavailable")
        }
        self.cancel_text();
        self.conn.res_query_version(1, 2)?.reply()?;
        let target_pid = self.application_pid(window)?;
        let bridge_pid = self.clipboard_bridge_pid();
        self.release_all();
        self.focus_for_input(window)?;
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
            self.prepare_copy_owner(owner)?;
            let interval = Self::clipboard_key_interval(&class, bridge_pid.is_some());
            self.copy_transfer = Some(CopyTransfer {
                owner,
                window_id: window,
                request_id: id,
                target_pid,
                bridge_pid,
                requested: false,
                cut,
                cut_sent: false,
                key_interval: interval,
                target: if rich {
                    None
                } else {
                    Some("UTF8_STRING".into())
                },
                incremental: false,
                bytes: Vec::new(),
                owner_deadline: Instant::now() + Duration::from_secs(3),
                deadline: Instant::now() + Duration::from_secs(if rich { 30 } else { 3 }),
            });
            self.clipboard_shortcut(window, b'c', terminal, interval)?;
            self.conn.flush()?;
            Ok(())
        })();
        if result.is_err() {
            self.release_all();
            self.cancel_copy();
            let _ = self.conn.destroy_window(owner);
        }
        result
    }
    fn request_copy_target(&self, t: &CopyTransfer) -> Result<()> {
        self.conn.convert_selection(
            t.owner,
            self.atom("CLIPBOARD"),
            self.atom(t.target.as_deref().unwrap_or("TARGETS")),
            self.atom("_LAN_REMOTEAPP_CLIPBOARD"),
            x11rb::CURRENT_TIME,
        )?;
        self.conn.flush()?;
        Ok(())
    }
    fn complete_copy(&self, t: &mut CopyTransfer, bytes: Vec<u8>) -> Result<Option<CopiedText>> {
        let kind = t.target.as_deref().context("clipboard target missing")?;
        let files = ["text/uri-list", "x-special/gnome-copied-files"].contains(&kind);
        let text = if kind == "UTF8_STRING" {
            let s = String::from_utf8(bytes.clone())?;
            remoteapp_protocol::validate_text(&s)?;
            s
        } else {
            String::new()
        };
        if kind == "image/png" {
            validate_png(&bytes)?;
        }
        if t.cut && !t.cut_sent {
            // File moves need destination acknowledgement; cross-host file
            // clipboard copies never delete or mark the original for deletion.
            if files {
                bail!("cross-host file cut unavailable")
            }
            if t.deadline.saturating_duration_since(Instant::now()) < Duration::from_secs(1) {
                bail!("clipboard cut preflight timed out")
            }
            self.known(t.window_id)?;
            self.focus_for_input(t.window_id)?;
            self.prepare_copy_owner(t.owner)?;
            t.requested = false;
            t.owner_deadline = Instant::now() + Duration::from_secs(3);
            t.cut_sent = true;
            t.incremental = false;
            t.bytes.clear();
            self.clipboard_shortcut(t.window_id, b'x', false, t.key_interval)?;
            return Ok(None);
        }
        let bundle = if files {
            Some(ClipboardStore::open()?.files(&bytes)?)
        } else if kind == "image/png" {
            Some(ClipboardStore::open()?.image(&bytes)?)
        } else {
            None
        };
        Ok(Some(CopiedText {
            window_id: t.window_id,
            request_id: t.request_id.clone(),
            text,
            bundle,
        }))
    }
    pub fn pump_copy(&mut self) -> Result<Option<CopiedText>> {
        let Some(mut t) = self.copy_transfer.take() else {
            return Ok(None);
        };
        let result = (|| {
            if !t.requested {
                if Instant::now() >= t.owner_deadline {
                    bail!("application did not offer clipboard contents")
                }
                let owner = self
                    .conn
                    .get_selection_owner(self.atom("CLIPBOARD"))?
                    .reply()?
                    .owner;
                if owner != 0 && owner != t.owner {
                    let pid = self.client_pid(owner)?;
                    if pid != t.target_pid && Some(pid) != t.bridge_pid {
                        bail!("clipboard owner does not belong to copied application")
                    }
                    self.request_copy_target(&t)?;
                    t.requested = true;
                }
            }
            for _ in 0..256 {
                let Some(event) = self.next_event()? else {
                    break;
                };
                match event {
                    Event::SelectionNotify(e)
                        if t.requested
                            && e.requestor == t.owner
                            && e.selection == self.atom("CLIPBOARD")
                            && e.target == self.atom(t.target.as_deref().unwrap_or("TARGETS")) =>
                    {
                        if e.property != self.atom("_LAN_REMOTEAPP_CLIPBOARD") {
                            bail!("application refused clipboard format")
                        }
                        let value = self
                            .conn
                            .get_property(
                                true,
                                t.owner,
                                e.property,
                                AtomEnum::ANY,
                                0,
                                (MAX_IMAGE_BYTES / 4 + 1) as u32,
                            )?
                            .reply()?;
                        if t.target.is_none() {
                            if value.format != 32
                                || value.type_ != u32::from(AtomEnum::ATOM)
                                || value.bytes_after != 0
                            {
                                bail!("invalid clipboard targets")
                            }
                            let atoms: Vec<_> = value
                                .value32()
                                .context("clipboard targets missing")?
                                .collect();
                            t.target = [
                                "x-special/gnome-copied-files",
                                "text/uri-list",
                                "image/png",
                                "UTF8_STRING",
                            ]
                            .into_iter()
                            .find(|n| atoms.contains(&self.atom(n)))
                            .map(String::from);
                            if t.target.is_none() {
                                bail!("application has no supported clipboard format")
                            }
                            self.request_copy_target(&t)?;
                            return Ok(None);
                        }
                        if value.type_ == self.atom("INCR") {
                            if value.format != 32
                                || value
                                    .value32()
                                    .and_then(|mut n| n.next())
                                    .is_none_or(|n| n as usize > MAX_IMAGE_BYTES)
                            {
                                bail!("clipboard INCR size exceeded")
                            }
                            t.incremental = true;
                        } else {
                            if value.format != 8
                                || value.type_ != self.atom(t.target.as_deref().unwrap())
                                || value.bytes_after != 0
                                || value.value.len() > MAX_IMAGE_BYTES
                            {
                                bail!("clipboard format or size exceeded")
                            }
                            return self.complete_copy(&mut t, value.value);
                        }
                    }
                    Event::PropertyNotify(e)
                        if t.incremental
                            && e.window == t.owner
                            && e.atom == self.atom("_LAN_REMOTEAPP_CLIPBOARD")
                            && e.state == Property::NEW_VALUE =>
                    {
                        let value = self
                            .conn
                            .get_property(
                                true,
                                t.owner,
                                e.atom,
                                AtomEnum::ANY,
                                0,
                                (MAX_IMAGE_BYTES / 4 + 1) as u32,
                            )?
                            .reply()?;
                        // A queued notification for the initial INCR property can
                        // precede the first actual chunk; its deleted property is absent.
                        if value.type_ == 0 {
                            continue;
                        }
                        if value.format != 8
                            || value.type_ != self.atom(t.target.as_deref().unwrap())
                            || value.bytes_after != 0
                            || t.bytes.len() + value.value.len() > MAX_IMAGE_BYTES
                        {
                            bail!("invalid clipboard INCR chunk")
                        }
                        if value.value.is_empty() {
                            t.incremental = false;
                            let bytes = std::mem::take(&mut t.bytes);
                            return self.complete_copy(&mut t, bytes);
                        }
                        t.bytes.extend(value.value);
                    }
                    Event::SelectionRequest(e) if e.owner == t.owner => {
                        self.conn.send_event(
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
                                property: 0,
                            },
                        )?;
                    }
                    _ => {}
                }
            }
            if Instant::now() >= t.deadline {
                bail!("application did not provide clipboard contents")
            }
            self.conn.flush()?;
            Ok(None)
        })();
        if matches!(&result, Ok(None)) {
            self.copy_transfer = Some(t)
        } else {
            self.release_all();
            let _ = self.conn.destroy_window(t.owner);
            let _ = self.conn.flush();
        }
        result
    }
}
