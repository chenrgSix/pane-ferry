//! Only operates on windows in the explicitly supplied dedicated fixture PID.
#![cfg(windows)]
use anyhow::{ensure, Context, Result};
use paneferry_backend_windows::{CaptureRuntime, Desktop, Input, Target, WindowSystem};
use std::time::{Duration, Instant};
use windows::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Dwm::*,
    UI::{Input::KeyboardAndMouse::IsWindowEnabled, WindowsAndMessaging::*},
};

fn capture(desktop: &mut Desktop, target: &Target) -> Result<()> {
    desktop.attach(target, 30)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(frame) = desktop.frame(target)? {
            ensure!(
                frame.key_frame && !frame.data.is_empty(),
                "missing dialog keyframe"
            );
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    anyhow::bail!("dialog capture timed out")
}

fn type_into_fixture(desktop: &mut Desktop, target: &Target, path: &str) -> Result<()> {
    let window = HWND(target.handle as *mut _);
    let mut child = RECT::default();
    let mut frame = RECT::default();
    unsafe {
        GetWindowRect(GetWindow(window, GW_CHILD)?, &mut child)?;
        DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut frame as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )?;
    }
    for pressed in [true, false] {
        desktop.input(
            target,
            Input::Pointer {
                x: (child.left + child.right) / 2 - frame.left,
                y: (child.top + child.bottom) / 2 - frame.top,
                button: 1,
                pressed,
            },
        )?;
    }
    let text = "Dialog input 中文🙂";
    desktop.input(target, Input::Text(text.into()))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if std::fs::read_to_string(path).ok().as_deref() == Some(text) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    anyhow::bail!("fixture did not receive dialog input")
}

#[test]
#[ignore = "run windows-dialogs.ps1 (optionally -FileDialog) and set PANEFERRY_DIALOG_FIXTURE_PID/RESULT"]
fn catalog_only_lists_parent_while_dialog_remains_in_composed_session() -> Result<()> {
    let pid: u32 = std::env::var("PANEFERRY_DIALOG_FIXTURE_PID")?.parse()?;
    let result = std::env::var("PANEFERRY_DIALOG_FIXTURE_RESULT")?;
    let _runtime = CaptureRuntime::new()?;
    let mut desktop = Desktop::new()?;
    let windows: Vec<_> = desktop
        .windows()?
        .into_iter()
        .filter(|w| w.pid == pid)
        .collect();
    ensure!(
        windows.len() == 1,
        "owned windows leaked into connection catalog"
    );
    let parent = windows.first().context("fixture parent missing")?;
    ensure!(
        parent.name == "PaneFerry dialog parent verification",
        "wrong session entry point"
    );
    let enabled = || unsafe { IsWindowEnabled(HWND(parent.handle as *mut _)).as_bool() };
    ensure!(!enabled(), "fixture modal dialog is not open");
    capture(&mut desktop, parent)?;
    // The dialog is absent from the catalog but still receives keyboard input
    // through the parent's composed session, including native file choosers.
    desktop.input(parent, Input::Focus)?;
    for pressed in [true, false] {
        desktop.input(
            parent,
            Input::Key {
                virtual_key: 0x1b,
                pressed,
            },
        )?;
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while !enabled() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    ensure!(
        enabled(),
        "dialog could not be dismissed through parent session"
    );
    capture(&mut desktop, parent)?;
    type_into_fixture(&mut desktop, parent, &format!("{result}.parent"))?;
    desktop.detach();
    Ok(())
}
