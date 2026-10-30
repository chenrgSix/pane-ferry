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
            desktop.detach();
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
#[ignore = "run windows-dialogs.ps1 and set PANEFERRY_DIALOG_FIXTURE_PID/RESULT; KIND=file for native chooser"]
fn owned_dialog_capture_input_close_and_parent_recovery() -> Result<()> {
    let pid: u32 = std::env::var("PANEFERRY_DIALOG_FIXTURE_PID")?.parse()?;
    let result = std::env::var("PANEFERRY_DIALOG_FIXTURE_RESULT")?;
    let file_dialog = std::env::var("PANEFERRY_DIALOG_FIXTURE_KIND").as_deref() == Ok("file");
    let _runtime = CaptureRuntime::new()?;
    let mut desktop = Desktop::new()?;
    let windows: Vec<_> = desktop
        .windows()?
        .into_iter()
        .filter(|w| w.pid == pid)
        .collect();
    let parent = windows
        .iter()
        .find(|w| w.name == "PaneFerry dialog parent verification")
        .context("fixture parent missing")?;
    let name = if file_dialog {
        "PaneFerry file chooser verification"
    } else {
        "PaneFerry dialog parent verification — 对话框"
    };
    let dialog = windows
        .iter()
        .find(|w| w.name == name)
        .context("owned dialog was filtered")?;
    ensure!(
        windows
            .iter()
            .any(|w| w.name == "PaneFerry owned child verification"),
        "owned child was filtered"
    );
    ensure!(
        !windows.iter().any(|w| w.name.contains("helper")),
        "helper leaked into catalog: {:?}",
        windows
            .iter()
            .filter(|w| w.name.contains("helper"))
            .map(|w| (&w.name, desktop.geometry(w)))
            .collect::<Vec<_>>()
    );
    ensure!(
        dialog.auto_connectable && dialog.application_key == parent.application_key,
        "dialog cannot follow application rules"
    );
    ensure!(
        desktop.geometry(dialog)?.title == name,
        "empty dialog title"
    );
    ensure!(
        unsafe { !IsWindowEnabled(HWND(parent.handle as *mut _)).as_bool() },
        "parent not modal-disabled"
    );
    ensure!(
        desktop
            .input(parent, Input::Text("must not be sent".into()))
            .is_err(),
        "disabled parent accepted input"
    );
    capture(&mut desktop, dialog)?;
    if !file_dialog {
        type_into_fixture(&mut desktop, dialog, &result)?;
    }
    desktop.input(dialog, Input::Focus)?;
    desktop.input(
        dialog,
        Input::Key {
            virtual_key: 0x1b,
            pressed: true,
        },
    )?;
    desktop.detach(); // Release Escape even if the dialog already destroyed its HWND.
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && desktop.geometry(dialog).is_ok() {
        std::thread::sleep(Duration::from_millis(20));
    }
    ensure!(
        desktop.geometry(dialog).is_err(),
        "closed dialog target still valid"
    );
    ensure!(
        !desktop.windows()?.iter().any(|w| w.id == dialog.id),
        "closed dialog still advertised"
    );
    capture(&mut desktop, parent)?;
    type_into_fixture(&mut desktop, parent, &format!("{result}.parent"))?;
    desktop.detach();
    Ok(())
}
