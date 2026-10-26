//! Window changes wake discovery; pixel damage does not require a full scan.
use crate::*;
use std::cell::Cell;
use x11rb::protocol::Event;

pub(super) struct MetadataEvents {
    enabled: Cell<bool>,
    dirty: Cell<bool>,
}
impl MetadataEvents {
    pub fn new(conn: &RustConnection, root: u32) -> Self {
        let enabled = (|| -> Result<()> {
            let attributes = conn.get_window_attributes(root)?.reply()?;
            conn.change_window_attributes(
                root,
                &ChangeWindowAttributesAux::new().event_mask(
                    attributes.your_event_mask
                        | EventMask::SUBSTRUCTURE_NOTIFY
                        | EventMask::PROPERTY_CHANGE,
                ),
            )?
            .check()?;
            Ok(())
        })()
        .is_ok();
        Self {
            enabled: Cell::new(enabled),
            dirty: Cell::new(false),
        }
    }
    pub fn enabled(&self) -> bool {
        self.enabled.get()
    }
    pub fn pending(&self) -> bool {
        self.dirty.get()
    }
    pub fn clear(&self) {
        self.dirty.set(false);
    }
    pub fn watch(&self, conn: &RustConnection, id: u32, selected: EventMask) {
        let wanted = EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE;
        if !self.enabled() || selected.contains(wanted) {
            return;
        }
        let result = (|| -> Result<()> {
            conn.change_window_attributes(
                id,
                &ChangeWindowAttributesAux::new().event_mask(selected | wanted),
            )?
            .check()?;
            Ok(())
        })();
        if let Err(error) = result {
            if error
                .downcast_ref::<x11rb::errors::ReplyError>()
                .is_some_and(|e| {
                    matches!(e, x11rb::errors::ReplyError::X11Error(e)
                    if e.error_kind == x11rb::protocol::ErrorKind::Window)
                })
            {
                self.dirty.set(true);
                return;
            }
            // A disappearing window must not disable discovery. Fall back to
            // the existing fast poll if notifications cannot be guaranteed.
            self.enabled.set(false);
        }
    }
    pub fn notify(
        &self,
        event: &Event,
        root: u32,
        windows: &HashMap<u32, Window>,
        atoms: &HashMap<String, u32>,
    ) {
        if !self.enabled() {
            return;
        }
        let tracked = |id| windows.contains_key(&id);
        let changed = match event {
            Event::MapNotify(e) => e.event == root || tracked(e.window),
            Event::UnmapNotify(e) => tracked(e.window),
            Event::DestroyNotify(e) => tracked(e.window),
            Event::ConfigureNotify(e) => tracked(e.window),
            Event::ReparentNotify(e) => e.parent == root || tracked(e.window),
            Event::PropertyNotify(e) if e.window == root => {
                ["_NET_CLIENT_LIST", "_NET_CLIENT_LIST_STACKING"]
                    .iter()
                    .any(|name| atoms.get(*name) == Some(&e.atom))
            }
            Event::PropertyNotify(e) if tracked(e.window) => [
                "_NET_WM_NAME",
                "WM_TRANSIENT_FOR",
                "_NET_WM_WINDOW_TYPE",
                "_NET_WM_STATE",
                "_NET_FRAME_EXTENTS",
                "WM_NORMAL_HINTS",
            ]
            .iter()
            .any(|name| atoms.get(*name) == Some(&e.atom)),
            _ => false,
        };
        if changed {
            self.dirty.set(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a private Xvfb DISPLAY with DAMAGE"]
    fn buffered_event_backlog_remains_ready_after_a_bounded_drain() -> Result<()> {
        use std::os::fd::AsRawFd;
        let (writer, screen) = x11rb::connect(None)?;
        let root = writer.setup().roots[screen].root;
        let mut backend = Backend::connect(&std::env::var("DISPLAY")?)?;
        let id = writer.generate_id()?;
        writer
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                id,
                root,
                0,
                0,
                320,
                240,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new(),
            )?
            .check()?;
        writer.map_window(id)?.check()?;
        writer
            .change_property32(
                PropMode::REPLACE,
                root,
                backend.atom("_NET_CLIENT_LIST"),
                AtomEnum::WINDOW,
                &[id],
            )?
            .check()?;
        backend.scan()?;
        backend.pump_capture_events()?;
        backend.capture(&backend.windows[&id])?;
        backend.pump_capture_events()?;
        backend.metadata.clear();
        let property = writer
            .intern_atom(false, b"_PANEFERRY_BUFFERED_EVENT_TEST")?
            .reply()?
            .atom;
        for value in 0..600u32 {
            writer.change_property32(
                PropMode::REPLACE,
                id,
                property,
                AtomEnum::CARDINAL,
                &[value],
            )?;
        }
        writer.get_input_focus()?.reply()?;
        backend.conn.get_input_focus()?.reply()?;
        backend.pump_capture_events()?;
        // All remaining notifications are already in x11rb, not the socket.
        let mut fd = libc::pollfd {
            fd: backend.conn.stream().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(unsafe { libc::poll(&mut fd, 1, 0) }, 0);
        assert!(
            backend.events_pending()?,
            "a drained socket must not hide buffered work"
        );
        assert!(
            backend.events_pending()?,
            "repeated readiness checks must not consume the event"
        );
        assert!(
            matches!(backend.next_event()?, Some(Event::PropertyNotify(e)) if e.window == id && e.atom == property)
        );
        assert!(
            !backend.metadata_ready(),
            "unrelated properties must not force full metadata scans"
        );
        for _ in 0..4 {
            backend.pump_capture_events()?;
        }
        assert!(
            !backend.events_pending()?,
            "once drained, the scheduler may sleep again"
        );
        writer.destroy_window(id)?.check()?;
        Ok(())
    }
    #[test]
    #[ignore = "requires a private Xvfb DISPLAY with DAMAGE; run with xvfb-run"]
    fn metadata_changes_wake_discovery_while_pixel_damage_only_wakes_capture() -> Result<()> {
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let mut backend = Backend::connect(&std::env::var("DISPLAY")?)?;
        assert!(backend.metadata_events());
        let id = conn.generate_id()?;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            id,
            root,
            10,
            20,
            320,
            240,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new(),
        )?
        .check()?;
        conn.map_window(id)?.check()?;
        conn.change_property32(
            PropMode::REPLACE,
            root,
            backend.atom("_NET_CLIENT_LIST"),
            AtomEnum::WINDOW,
            &[id],
        )?
        .check()?;
        backend.pump_capture_events()?;
        assert!(backend.metadata_ready(), "a new map must wake discovery");
        backend.scan()?;
        assert!(backend.windows.contains_key(&id));
        backend.pump_capture_events()?;
        backend.metadata.clear();
        backend.damage.borrow_mut().acknowledge(&backend.conn, id)?;

        let gc = conn.generate_id()?;
        conn.create_gc(gc, id, &CreateGCAux::new())?.check()?;
        conn.poly_fill_rectangle(
            id,
            gc,
            &[Rectangle {
                x: 1,
                y: 1,
                width: 10,
                height: 10,
            }],
        )?
        .check()?;
        backend.pump_capture_events()?;
        assert!(backend.capture_needed(id));
        assert!(
            !backend.metadata_ready(),
            "painting does not require a window scan"
        );

        conn.configure_window(id, &ConfigureWindowAux::new().width(400))?
            .check()?;
        backend.pump_capture_events()?;
        assert!(backend.metadata_ready());
        backend.scan()?;
        assert_eq!(backend.windows[&id].width, 400);
        backend.pump_capture_events()?;
        backend.metadata.clear();

        conn.change_property8(
            PropMode::REPLACE,
            id,
            backend.atom("_NET_WM_NAME"),
            backend.atom("UTF8_STRING"),
            b"updated fixture title",
        )?
        .check()?;
        backend.pump_capture_events()?;
        assert!(backend.metadata_ready());
        backend.scan()?;
        assert_eq!(backend.windows[&id].title, "updated fixture title");
        backend.pump_capture_events()?;
        backend.metadata.clear();

        conn.destroy_window(id)?.check()?;
        backend.pump_capture_events()?;
        assert!(backend.metadata_ready());
        backend.scan()?;
        assert!(!backend.windows.contains_key(&id));
        Ok(())
    }
}
