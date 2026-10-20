//! Copy only after an explicit command to the currently controlled application.
use super::*;
use std::time::{Duration, Instant};
use x11rb::protocol::{res::ConnectionExt as _, Event};

pub(super) struct CopyTransfer {
    owner: u32,
    window_id: u32,
    request_id: String,
    target_pid: u32,
    bridge_pid: Option<u32>,
    requested: bool,
    deadline: Instant,
}
pub struct CopiedText {
    pub window_id: u32,
    pub request_id: String,
    pub text: String,
}
impl Backend {
    pub(super) fn cancel_copy(&mut self) {
        if let Some(transfer) = self.copy_transfer.take() {
            let _ = self.conn.destroy_window(transfer.owner);
            let _ = self.conn.flush();
        }
    }
    pub fn copy_text(&mut self, window_id: u32, request_id: String) -> Result<()> {
        self.known(window_id)?;
        if self.text_pending() {
            bail!("clipboard operation already pending")
        }
        self.cancel_text();
        self.conn.res_query_version(1, 2)?.reply()?;
        let target_pid = self.application_pid(window_id)?;
        let bridge_pid = self.clipboard_bridge_pid();
        self.release_all();
        self.focus(window_id)?;
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
            // Taking ownership first prevents returning old unrelated clipboard
            // contents when Ctrl+C has no current selection to copy.
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
            self.copy_transfer = Some(CopyTransfer {
                owner,
                window_id,
                request_id,
                target_pid,
                bridge_pid,
                requested: false,
                deadline: Instant::now() + Duration::from_secs(3),
            });
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
            self.clipboard_shortcut(window_id, b'c', terminal)?;
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
    pub fn pump_copy(&mut self) -> Result<Option<CopiedText>> {
        let Some(mut transfer) = self.copy_transfer.take() else {
            return Ok(None);
        };
        let result = (|| {
            if !transfer.requested {
                let owner = self
                    .conn
                    .get_selection_owner(self.atom("CLIPBOARD"))?
                    .reply()?
                    .owner;
                if owner != 0 && owner != transfer.owner {
                    let pid = self.client_pid(owner)?;
                    if pid != transfer.target_pid && Some(pid) != transfer.bridge_pid {
                        bail!("clipboard owner does not belong to copied application")
                    }
                    self.conn.convert_selection(
                        transfer.owner,
                        self.atom("CLIPBOARD"),
                        self.atom("UTF8_STRING"),
                        self.atom("_LAN_REMOTEAPP_CLIPBOARD"),
                        x11rb::CURRENT_TIME,
                    )?;
                    self.conn.flush()?;
                    transfer.requested = true;
                }
            }
            for _ in 0..256 {
                let Some(event) = self.conn.poll_for_event()? else {
                    break;
                };
                match event {
                    Event::SelectionNotify(e)
                        if transfer.requested
                            && e.requestor == transfer.owner
                            && e.selection == self.atom("CLIPBOARD")
                            && e.target == self.atom("UTF8_STRING") =>
                    {
                        if e.property != self.atom("_LAN_REMOTEAPP_CLIPBOARD") {
                            bail!("application refused text copy")
                        }
                        let value = self
                            .conn
                            .get_property(
                                true,
                                transfer.owner,
                                e.property,
                                AtomEnum::ANY,
                                0,
                                (remoteapp_protocol::MAX_TEXT_BYTES / 4 + 1) as u32,
                            )?
                            .reply()?;
                        if value.format != 8
                            || value.bytes_after != 0
                            || value.value.len() > remoteapp_protocol::MAX_TEXT_BYTES
                            || value.type_ != self.atom("UTF8_STRING")
                        {
                            bail!("copied text is unsupported or too large")
                        }
                        let text = String::from_utf8(value.value)?;
                        remoteapp_protocol::validate_text(&text)?;
                        return Ok(Some(CopiedText {
                            window_id: transfer.window_id,
                            request_id: transfer.request_id.clone(),
                            text,
                        }));
                    }
                    Event::SelectionRequest(e) if e.owner == transfer.owner => {
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
            if Instant::now() >= transfer.deadline {
                bail!("application did not copy selected text")
            }
            self.conn.flush()?;
            Ok(None)
        })();
        if matches!(&result, Ok(None)) {
            self.copy_transfer = Some(transfer);
        } else {
            let _ = self.conn.destroy_window(transfer.owner);
            let _ = self.conn.flush();
        }
        result
    }
}
