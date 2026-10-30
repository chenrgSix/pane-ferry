use super::*;
use std::time::{Duration, Instant};

#[test]
#[ignore = "read-only real application capture; set PANEFERRY_SCENE_ANCHOR_ID/CHILD_ID"]
fn existing_application_and_dialog_share_one_encoded_scene() -> Result<()> {
    let anchor_id = std::env::var("PANEFERRY_SCENE_ANCHOR_ID")?;
    let child_id = std::env::var("PANEFERRY_SCENE_CHILD_ID")?;
    let _runtime = CaptureRuntime::new()?;
    let mut desktop = Desktop::new()?;
    let anchor = desktop
        .windows()?
        .into_iter()
        .find(|w| w.id == anchor_id)
        .context("requested anchor missing")?;
    desktop.attach(&anchor, 30)?;
    wait_scene(&mut desktop, |scene, _| {
        scene
            .layout
            .surfaces
            .iter()
            .any(|s| s.target.id == child_id)
            && scene
                .layers
                .get(&child_id)
                .is_some_and(|l| l.pixels.is_some())
    })?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(frame) = desktop.frame(&anchor)? {
            ensure!(
                frame.key_frame && !frame.data.is_empty(),
                "missing combined H.264 keyframe"
            );
            eprintln!(
                "Composed {} surfaces into {}x{}; H.264 keyframe {} bytes",
                desktop.scene.as_ref().unwrap().layout.surfaces.len(),
                frame.width,
                frame.height,
                frame.data.len()
            );
            desktop.detach();
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    bail!("combined video timed out")
}

#[test]
fn composition_handles_negative_offsets_alpha_and_downscaling() {
    let rect = RECT {
        left: -100,
        top: -50,
        right: 100,
        bottom: 50,
    };
    let layout = Layout {
        rect,
        width: 100,
        height: 50,
        surfaces: vec![],
    };
    let mut canvas = vec![0; 100 * 50 * 4];
    paint(
        &layout,
        rect,
        200,
        100,
        &[0, 0, 255, 255].repeat(200 * 100),
        &mut canvas,
    );
    let overlay = RECT {
        left: -50,
        top: -25,
        right: 50,
        bottom: 25,
    };
    paint(
        &layout,
        overlay,
        100,
        50,
        &[0, 128, 0, 128].repeat(100 * 50),
        &mut canvas,
    );
    assert_eq!(&canvas[0..4], &[0, 0, 255, 255]);
    assert_eq!(&canvas[(25 * 100 + 50) * 4..][..4], &[0, 128, 127, 255]);
    let (w, h) = sized(RECT {
        left: -10000,
        top: -5000,
        right: 10000,
        bottom: 5000,
    });
    assert!(paneferry_protocol::dimensions(w, h).is_ok());
}

#[test]
#[ignore = "requires windows-menus.ps1 fixture and PANEFERRY_MENU_FIXTURE_PID/RESULT"]
fn menu_pixels_and_pointer_are_part_of_parent_capture() -> Result<()> {
    let pid: u32 = std::env::var("PANEFERRY_MENU_FIXTURE_PID")?.parse()?;
    let result = std::env::var("PANEFERRY_MENU_FIXTURE_RESULT")?;
    let _runtime = CaptureRuntime::new()?;
    let mut desktop = Desktop::new()?;
    let target = desktop
        .windows()?
        .into_iter()
        .find(|w| w.pid == pid && w.name == "PaneFerry menu verification")
        .context("dedicated menu fixture missing")?;
    desktop.attach(&target, 30)?;
    desktop.input(&target, Input::Focus)?;
    let focus_deadline = Instant::now() + Duration::from_secs(2);
    while unsafe { GetForegroundWindow() } != hwnd(&target) && Instant::now() < focus_deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    ensure!(
        unsafe { GetForegroundWindow() } == hwnd(&target),
        "fixture did not obtain foreground"
    );
    for pressed in [true, false] {
        desktop.input(
            &target,
            Input::Pointer {
                x: 100,
                y: 100,
                button: 3,
                pressed,
            },
        )?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut menu_pixels = false;
    while Instant::now() < deadline {
        if let Some((_, _, pixels)) = desktop.scene.as_mut().unwrap().frame()? {
            // The fixture background is blue; only its popup paints magenta.
            menu_pixels = pixels
                .chunks_exact(4)
                .filter(|p| p[0] > 230 && p[1] < 25 && p[2] > 230)
                .count()
                > 100;
            if menu_pixels {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    ensure!(menu_pixels, "popup pixels absent from parent capture");
    let values: Vec<i32> = std::fs::read_to_string(format!("{result}.bounds"))?
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    ensure!(values.len() == 4, "invalid fixture menu bounds");
    let scene = desktop.scene.as_ref().unwrap();
    ensure!(
        scene.layout.surfaces.len() >= 2,
        "menu did not join source scene"
    );
    let rect = scene.layout.rect;
    let x = ((values[0] + values[2] / 2 - rect.left) as i64 * scene.layout.width as i64
        / (rect.right - rect.left) as i64) as i32;
    let y = ((values[1] + 12 - rect.top) as i64 * scene.layout.height as i64
        / (rect.bottom - rect.top) as i64) as i32;
    desktop.input(
        &target,
        Input::Pointer {
            x,
            y,
            button: 0,
            pressed: false,
        },
    )?;
    for pressed in [true, false] {
        desktop.input(
            &target,
            Input::Pointer {
                x,
                y,
                button: 1,
                pressed,
            },
        )?;
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if std::fs::read_to_string(&result).ok().as_deref() == Some("clicked") {
            return verify_new_window(&mut desktop, &target, &result);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    desktop.detach();
    bail!("menu item did not receive pointer input")
}

fn canvas_point(scene: &Scene, point: POINT) -> (i32, i32) {
    let l = &scene.layout;
    (
        ((point.x - l.rect.left) as i64 * l.width as i64 / (l.rect.right - l.rect.left) as i64)
            as i32,
        ((point.y - l.rect.top) as i64 * l.height as i64 / (l.rect.bottom - l.rect.top) as i64)
            as i32,
    )
}
fn click(desktop: &mut Desktop, anchor: &Target, x: i32, y: i32) -> Result<()> {
    for pressed in [true, false] {
        desktop.input(
            anchor,
            Input::Pointer {
                x,
                y,
                button: 1,
                pressed,
            },
        )?;
    }
    Ok(())
}
fn wait_scene(desktop: &mut Desktop, predicate: impl Fn(&Scene, &[u8]) -> bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        let scene = desktop.scene.as_mut().unwrap();
        if let Some((_, _, pixels)) = scene.frame()? {
            if predicate(scene, &pixels) {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    bail!("composed scene did not reach expected state")
}
fn verify_new_window(desktop: &mut Desktop, anchor: &Target, result: &str) -> Result<()> {
    let ready = Instant::now() + Duration::from_secs(2);
    while !std::path::Path::new(&format!("{result}.ready")).exists() && Instant::now() < ready {
        std::thread::sleep(Duration::from_millis(20));
    }
    ensure!(
        std::path::Path::new(&format!("{result}.ready")).exists(),
        "fixture new-window callback has not finished"
    );
    wait_scene(desktop, |s, p| {
        s.layout
            .surfaces
            .iter()
            .any(|w| w.target.name == "PaneFerry new scene window")
            && p.chunks_exact(4)
                .filter(|c| c[2] > 230 && c[0] < 25 && c[1] < 25)
                .count()
                > 100
    })?;
    let scene = desktop.scene.as_ref().unwrap();
    let child = scene
        .layout
        .surfaces
        .iter()
        .find(|w| w.target.name == "PaneFerry new scene window")
        .unwrap();
    ensure!(
        scene.layout.width > window_geometry(anchor)?.width,
        "outside window clipped to anchor"
    );
    let (x, y) = canvas_point(
        scene,
        POINT {
            x: (child.rect.left + child.rect.right) / 2,
            y: (child.rect.top + child.rect.bottom) / 2,
        },
    );
    let parent_rect = bounds(anchor)?;
    let gap = canvas_point(
        scene,
        POINT {
            x: (parent_rect.right + child.rect.left) / 2,
            y: parent_rect.top + 50,
        },
    );
    desktop.input(
        anchor,
        Input::Pointer {
            x: gap.0,
            y: gap.1,
            button: 0,
            pressed: false,
        },
    )?;
    click(desktop, anchor, x, y)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while std::fs::read_to_string(result).ok().as_deref() != Some("child-clicked")
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    ensure!(
        std::fs::read_to_string(result)? == "child-clicked",
        "composed child click missed"
    );
    for pressed in [true, false] {
        desktop.input(
            anchor,
            Input::Key {
                virtual_key: 0x1b,
                pressed,
            },
        )?;
    }
    wait_scene(desktop, |s, _| s.layout.surfaces.len() == 1)?;
    ensure!(
        desktop.geometry(anchor)?.width == window_geometry(anchor)?.width,
        "canvas did not contract"
    );
    desktop.detach();
    Ok(())
}

#[test]
#[ignore = "run windows-dialogs.ps1 -Composite with PANEFERRY_DIALOG_FIXTURE_PID/RESULT"]
fn modal_is_composed_outside_parent_and_receives_parent_stream_input() -> Result<()> {
    let pid: u32 = std::env::var("PANEFERRY_DIALOG_FIXTURE_PID")?.parse()?;
    let result = std::env::var("PANEFERRY_DIALOG_FIXTURE_RESULT")?;
    let _runtime = CaptureRuntime::new()?;
    let mut desktop = Desktop::new()?;
    let anchor = desktop
        .windows()?
        .into_iter()
        .find(|w| w.pid == pid && w.name == "PaneFerry dialog parent verification")
        .context("fixture parent missing")?;
    desktop.attach(&anchor, 30)?;
    wait_scene(&mut desktop, |_, p| {
        p.chunks_exact(4)
            .filter(|c| c[1] > 230 && c[0] < 25 && c[2] < 25)
            .count()
            > 100
    })?;
    let scene = desktop.scene.as_ref().unwrap();
    let dialog=scene.layout.surfaces.iter().find(|w|unsafe{GetWindow(hwnd(&w.target),GW_OWNER).unwrap_or_default()==hwnd(&anchor)}
        && w.target.name.is_empty()).context("modal not in composed scene")?;
    ensure!(
        scene.layout.width > window_geometry(&anchor)?.width
            && scene.layout.rect.top < bounds(&anchor)?.top,
        "dialog outside parent was cropped"
    );
    let mut other = Desktop::new()?;
    ensure!(
        other.attach(&dialog.target, 30).is_err(),
        "composed surface acquired twice"
    );
    let mut box_rect = RECT::default();
    unsafe {
        GetWindowRect(GetWindow(hwnd(&dialog.target), GW_CHILD)?, &mut box_rect)?;
    }
    let (x, y) = canvas_point(
        scene,
        POINT {
            x: (box_rect.left + box_rect.right) / 2,
            y: (box_rect.top + box_rect.bottom) / 2,
        },
    );
    click(&mut desktop, &anchor, x, y)?;
    desktop.input(&anchor, Input::Text("Composed 中文🙂".into()))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while std::fs::read_to_string(&result).ok().as_deref() != Some("Composed 中文🙂")
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    ensure!(
        std::fs::read_to_string(&result)? == "Composed 中文🙂",
        "modal input did not route through anchor"
    );
    for pressed in [true, false] {
        desktop.input(
            &anchor,
            Input::Key {
                virtual_key: 0x1b,
                pressed,
            },
        )?;
    }
    wait_scene(&mut desktop, |s, _| s.layout.surfaces.len() == 1)?;
    desktop.input(&anchor, Input::Text("parent restored".into()))?;
    desktop.detach();
    Ok(())
}
