//! Coalesced dirty notifications for already-authorized visible windows only.
use anyhow::Result;
use paneferry_protocol::Window;
use std::collections::HashMap;
use x11rb::{
    connection::Connection,
    protocol::damage::{ConnectionExt as _, NotifyEvent, ReportLevel},
    rust_connection::RustConnection,
};

struct Tracked {
    damage: Option<u32>,
    revision: u64,
    dirty: bool,
    rearmed_at: u64,
}
pub(super) struct Damage {
    enabled: bool,
    windows: HashMap<u32, Tracked>,
}
impl Damage {
    pub fn new(connection: &RustConnection) -> Self {
        let enabled = (|| -> Result<()> {
            connection.damage_query_version(1, 1)?.reply()?;
            Ok(())
        })()
        .is_ok();
        Self {
            enabled,
            windows: HashMap::new(),
        }
    }
    pub fn sync(
        &mut self,
        connection: &RustConnection,
        windows: &HashMap<u32, Window>,
    ) -> Result<()> {
        let removed: Vec<_> = self
            .windows
            .keys()
            .copied()
            .filter(|id| !windows.get(id).is_some_and(|w| w.visible))
            .collect();
        for id in removed {
            if let Some(damage) = self.windows.remove(&id).and_then(|w| w.damage) {
                if let Ok(cookie) = connection.damage_destroy(damage) {
                    let _ = cookie.check();
                }
            }
        }
        for window in windows.values().filter(|w| w.visible) {
            if let Some(tracked) = self.windows.get_mut(&window.window_id) {
                if tracked.revision != window.geometry_revision {
                    tracked.dirty = true;
                    tracked.revision = window.geometry_revision;
                }
                continue;
            }
            let damage = if self.enabled {
                let id = connection.generate_id()?;
                connection
                    .damage_create(id, window.window_id, ReportLevel::NON_EMPTY)?
                    .check()
                    .ok()
                    .map(|_| id)
            } else {
                None
            };
            self.windows.insert(
                window.window_id,
                Tracked {
                    damage,
                    revision: window.geometry_revision,
                    dirty: true,
                    rearmed_at: 0,
                },
            );
        }
        Ok(())
    }
    pub fn notify(&mut self, event: &NotifyEvent, sequence: u64) {
        if let Some(window) = self.windows.get_mut(&event.drawable) {
            if window.damage == Some(event.damage) && sequence >= window.rearmed_at {
                window.dirty = true;
            }
        }
    }
    pub fn pending(&self) -> bool {
        self.windows.values().any(|w| w.dirty || w.damage.is_none())
    }
    pub fn needed(&self, window: u32) -> bool {
        self.windows
            .get(&window)
            .is_none_or(|w| w.dirty || w.damage.is_none())
    }
    pub fn acknowledge(&mut self, connection: &RustConnection, window: u32) -> Result<()> {
        if let Some(tracked) = self.windows.get_mut(&window) {
            if let Some(id) = tracked.damage {
                // Rearm before reading pixels; any painting during/after this
                // capture will issue another notification, so it cannot be lost.
                let request = connection.damage_subtract(id, 0u32, 0u32)?;
                let sequence = request.sequence_number();
                request.check()?;
                // Replies can buffer an older notification until after this
                // readback. It describes pixels already covered by the snapshot.
                // Equal/newer sequence numbers belong to the rearmed region.
                tracked.rearmed_at = sequence;
                tracked.dirty = false;
            }
        }
        Ok(())
    }
    pub fn retry(&mut self, window: u32) {
        if let Some(w) = self.windows.get_mut(&window) {
            w.dirty = true;
        }
    }
    pub fn refresh(&mut self) {
        for window in self.windows.values_mut() {
            window.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::protocol::{
        xproto::{ConnectionExt as _, CreateGCAux, CreateWindowAux, Rectangle, WindowClass},
        Event,
    };
    #[test]
    fn rearm_sequence_keeps_new_damage_across_16_bit_wrap() {
        let mut tracker = Damage {
            enabled: true,
            windows: HashMap::from([(
                42,
                Tracked {
                    damage: Some(7),
                    revision: 1,
                    dirty: false,
                    rearmed_at: 65_540,
                },
            )]),
        };
        let event = NotifyEvent {
            drawable: 42,
            damage: 7,
            sequence: 3,
            ..Default::default()
        };
        tracker.notify(&event, 65_539);
        assert!(!tracker.pending());
        tracker.notify(&event, 65_540);
        assert!(
            tracker.pending(),
            "damage generated after rearm can share its request sequence"
        );
    }
    #[test]
    #[ignore = "requires a private Xvfb DISPLAY with DAMAGE and MIT-SHM"]
    fn buffered_damage_covered_by_a_snapshot_does_not_request_another_capture() -> Result<()> {
        use x11rb::protocol::xproto::{AtomEnum, PropMode};
        use x11rb::wrapper::ConnectionExt as _;
        let (painter, screen) = x11rb::connect(None)?;
        let root = painter.setup().roots[screen].root;
        let mut backend = crate::Backend::connect(&std::env::var("DISPLAY")?)?;
        let id = painter.generate_id()?;
        painter
            .create_window(
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
        painter.map_window(id)?.check()?;
        painter
            .change_property32(
                PropMode::REPLACE,
                root,
                backend.atom("_NET_CLIENT_LIST"),
                AtomEnum::WINDOW,
                &[id],
            )?
            .check()?;
        backend.scan()?;
        let window = backend.windows[&id].clone();
        backend.pump_capture_events()?;
        backend.capture(&window)?;
        backend.pump_capture_events()?;
        assert!(!backend.capture_pending());
        let gc = painter.generate_id()?;
        painter
            .create_gc(gc, id, &CreateGCAux::new().foreground(0xff0000))?
            .check()?;
        let paint = || -> Result<()> {
            painter
                .poly_fill_rectangle(
                    id,
                    gc,
                    &[Rectangle {
                        x: 0,
                        y: 0,
                        width: 320,
                        height: 240,
                    }],
                )?
                .check()?;
            Ok(())
        };
        paint()?;
        // A synchronous reply pulls the older Damage notification into the
        // client library. Deliberately leave it there until after the snapshot.
        backend.conn.get_input_focus()?.reply()?;
        let image = backend.capture(&window)?;
        assert_eq!(&image.data[..3], &[0, 0, 255]);
        backend.pump_capture_events()?;
        assert!(!backend.capture_pending(), "a queued pre-readback event already covered by the snapshot must not spend another frame interval");
        painter
            .change_gc(
                gc,
                &x11rb::protocol::xproto::ChangeGCAux::new().foreground(0x00ff00),
            )?
            .check()?;
        paint()?;
        backend.conn.get_input_focus()?.reply()?;
        backend.pump_capture_events()?;
        assert!(
            backend.capture_needed(id),
            "painting after rearm must remain pending"
        );
        let next = backend.capture(&window)?;
        assert_eq!(&next.data[..3], &[0, 255, 0]);
        painter.free_gc(gc)?;
        painter.destroy_window(id)?.check()?;
        Ok(())
    }
    #[test]
    #[ignore = "requires a private Xvfb DISPLAY with DAMAGE; run with xvfb-run"]
    fn dirty_windows_rearm_and_ignore_unapproved_drawables() {
        let (connection, screen) = x11rb::connect(None).unwrap();
        let root = connection.setup().roots[screen].root;
        let mut ids = Vec::new();
        for index in 0..2 {
            let id = connection.generate_id().unwrap();
            connection
                .create_window(
                    x11rb::COPY_DEPTH_FROM_PARENT,
                    id,
                    root,
                    index * 660,
                    0,
                    if index == 0 { 640 } else { 320 },
                    480,
                    0,
                    WindowClass::INPUT_OUTPUT,
                    0,
                    &CreateWindowAux::new(),
                )
                .unwrap()
                .check()
                .unwrap();
            connection.map_window(id).unwrap().check().unwrap();
            ids.push(id);
        }
        let window = Window {
            window_id: ids[0],
            parent_id: None,
            title: String::new(),
            presentation: None,
            role: "toplevel".into(),
            x: 0,
            y: 0,
            width: 640,
            height: 480,
            scale_factor: 1,
            geometry_revision: 1,
            visible: true,
        };
        let mut windows = HashMap::from([(window.window_id, window)]);
        let mut tracker = Damage::new(&connection);
        assert!(tracker.enabled);
        tracker.sync(&connection, &windows).unwrap();
        let pump = |tracker: &mut Damage| {
            connection.get_input_focus().unwrap().reply().unwrap();
            while let Some((event, sequence)) = connection.poll_for_event_with_sequence().unwrap() {
                if let Event::DamageNotify(event) = event {
                    tracker.notify(&event, sequence);
                }
            }
        };
        pump(&mut tracker);
        assert!(tracker.needed(ids[0]));
        tracker.acknowledge(&connection, ids[0]).unwrap();
        pump(&mut tracker);
        assert!(!tracker.pending());
        let gc = connection.generate_id().unwrap();
        connection
            .create_gc(gc, root, &CreateGCAux::new().foreground(0x12ab34))
            .unwrap()
            .check()
            .unwrap();
        let paint = |id| {
            connection
                .poly_fill_rectangle(
                    id,
                    gc,
                    &[Rectangle {
                        x: 0,
                        y: 0,
                        width: 64,
                        height: 48,
                    }],
                )
                .unwrap()
                .check()
                .unwrap();
        };
        paint(ids[1]);
        pump(&mut tracker);
        assert!(!tracker.pending(), "unapproved window triggered capture");
        for _ in 0..2 {
            paint(ids[0]);
            pump(&mut tracker);
            assert!(tracker.pending());
            tracker.acknowledge(&connection, ids[0]).unwrap();
            pump(&mut tracker);
            assert!(!tracker.pending());
        }
        tracker.refresh();
        assert!(
            tracker.needed(ids[0]),
            "a new controller needs a static snapshot"
        );
        tracker.acknowledge(&connection, ids[0]).unwrap();
        pump(&mut tracker);
        assert!(!tracker.pending());
        windows.get_mut(&ids[0]).unwrap().geometry_revision += 1;
        tracker.sync(&connection, &windows).unwrap();
        assert!(tracker.needed(ids[0]));
        windows.get_mut(&ids[0]).unwrap().visible = false;
        tracker.sync(&connection, &windows).unwrap();
        assert!(!tracker.pending());
        assert!(tracker.windows.is_empty());
        for id in ids {
            connection.destroy_window(id).unwrap().check().unwrap();
        }
    }
    #[test]
    #[ignore = "requires a private Xvfb started with -extension DAMAGE"]
    fn unavailable_extension_keeps_polling_fallback() {
        let (connection, _) = x11rb::connect(None).unwrap();
        let mut tracker = Damage::new(&connection);
        assert!(!tracker.enabled);
        let window = Window {
            window_id: 42,
            parent_id: None,
            title: String::new(),
            presentation: None,
            role: "toplevel".into(),
            x: 0,
            y: 0,
            width: 640,
            height: 480,
            scale_factor: 1,
            geometry_revision: 1,
            visible: true,
        };
        tracker
            .sync(&connection, &HashMap::from([(42, window)]))
            .unwrap();
        assert!(tracker.pending() && tracker.needed(42));
        tracker.acknowledge(&connection, 42).unwrap();
        assert!(tracker.pending() && tracker.needed(42));
    }
}
