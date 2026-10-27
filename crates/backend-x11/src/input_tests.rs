#![cfg(target_os = "linux")]
use super::*;
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Mutex,
};

#[test]
#[ignore = "requires a private Xvfb DISPLAY; run with xvfb-run"]
fn compositor_point_validation_and_owned_releases_keep_input_boundaries() -> Result<()> {
    struct Input(Arc<Mutex<Vec<String>>>);
    impl DesktopInput for Input {
        fn prepare(&self) {}
        fn status(&self) -> &'static str {
            "ready"
        }
        fn pointer(&self, x: i32, y: i32, _: u8, pressed: bool) -> Result<()> {
            assert_eq!((x, y), (12, 34));
            self.0.lock().unwrap().push(format!("pointer:{pressed}"));
            Ok(())
        }
        fn key(&self, _: u8, pressed: bool) -> Result<()> {
            self.0.lock().unwrap().push(format!("key:{pressed}"));
            Ok(())
        }
        fn sync(&self) -> Result<()> {
            Ok(())
        }
        fn release_all(&self) {
            self.0.lock().unwrap().push("release".into());
        }
    }
    struct Binding {
        events: Arc<Mutex<Vec<String>>>,
        mode: Arc<AtomicU8>,
    }
    impl DesktopGeometry for Binding {
        fn input_point(
            &self,
            _: u32,
            point: (i32, i32),
            size: (u32, u32),
            focus: bool,
        ) -> Result<Option<(i32, i32)>> {
            assert_eq!(point, (1, 2));
            assert_eq!(size, (100, 100));
            self.events.lock().unwrap().push(format!("point:{focus}"));
            match self.mode.load(Ordering::Relaxed) {
                0 => Ok(None),
                1 => bail!("replaced object"),
                _ => Ok(Some((12, 34))),
            }
        }
        fn input_target(&self, _: u32, focus: bool) -> Result<bool> {
            self.events.lock().unwrap().push(format!("target:{focus}"));
            bail!("replaced object")
        }
        fn resize(&self, _: u32, _: u32, _: u32) -> Result<bool> {
            Ok(false)
        }
        fn pointer(&self, _: u32, _: i32, _: i32, _: u32, _: u32) -> Result<Option<(i32, i32)>> {
            panic!("an accepted/rejected bound point must never retry another route")
        }
    }
    let (conn, screen) = x11rb::connect(None)?;
    let mut backend = Backend::from_connection(conn, screen, None)?;
    let id = backend.conn.generate_id()?;
    backend.window_scope = Some(related_windows::Scope::new(
        u16::MAX,
        std::process::id(),
        id,
        &[],
    )?);
    backend.windows.insert(
        id,
        Window {
            window_id: id,
            parent_id: None,
            title: String::new(),
            role: "toplevel".into(),
            presentation: None,
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            geometry_revision: 1,
            scale_factor: 1,
            visible: true,
        },
    );
    let events = Arc::new(Mutex::new(vec![]));
    let mode = Arc::new(AtomicU8::new(2));
    backend.set_desktop_input(Arc::new(Input(events.clone())));
    backend.set_desktop_geometry(Box::new(Binding {
        events: events.clone(),
        mode: mode.clone(),
    }));
    backend.pointer(id, 1, 2, 0, false)?;
    backend.pointer(id, 1, 2, 1, true)?;
    backend.pointer(id, 1, 2, 1, false)?;
    backend.key(id, 0xff51, false)?;
    assert!(
        backend.key(id, 0xff51, true).is_err(),
        "key-down must retain target/focus validation"
    );
    assert_eq!(
        *events.lock().unwrap(),
        [
            "point:false",
            "pointer:false",
            "point:true",
            "pointer:true",
            "point:false",
            "pointer:false",
            "key:false",
            "target:true"
        ]
    );
    events.lock().unwrap().clear();
    backend.windows.get_mut(&id).unwrap().visible = false;
    assert!(backend.pointer(id, 1, 2, 1, true).is_err());
    assert!(backend.pointer(id + 1, 1, 2, 1, true).is_err());
    assert!(
        events.lock().unwrap().is_empty(),
        "hidden/unknown presses cannot reach the compositor or broker"
    );
    backend.windows.get_mut(&id).unwrap().visible = true;
    mode.store(1, Ordering::Relaxed);
    assert!(backend.pointer(id, 1, 2, 0, false).is_err());
    backend.pointer(id, 1, 2, 1, false)?;
    assert_eq!(
        *events.lock().unwrap(),
        ["point:false", "point:false", "release"]
    );
    events.lock().unwrap().clear();
    mode.store(0, Ordering::Relaxed);
    assert!(backend.pointer(id, 1, 2, 0, false).is_err());
    assert_eq!(
        *events.lock().unwrap(),
        ["point:false", "target:false"],
        "declining fast validation must retain the original route"
    );
    Ok(())
}
