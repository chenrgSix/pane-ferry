//! Opt-in input test restricted to the dedicated windows-input.ps1 fixture.
#![cfg(windows)]
use anyhow::{ensure, Context, Result};
use paneferry_backend_windows::{CaptureRuntime, Desktop, Input, WindowSystem};
use std::time::{Duration, Instant};
use windows::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
    UI::WindowsAndMessaging::{
        FindWindowW, GetAncestor, GetCursorPos, GetForegroundWindow, GetGUIThreadInfo, GetWindow,
        GetWindowRect, GetWindowThreadProcessId, SetForegroundWindow, WindowFromPoint, GA_ROOT,
        GUITHREADINFO, GW_CHILD,
    },
};

#[test]
#[ignore = "run windows-input.ps1, then set PANEFERRY_INPUT_FIXTURE_PID and PANEFERRY_INPUT_FIXTURE_RESULT"]
fn click_activates_background_fixture_then_accepts_unicode_text() -> Result<()> {
    let pid: u32 = std::env::var("PANEFERRY_INPUT_FIXTURE_PID")?.parse()?;
    let result_path = std::env::var("PANEFERRY_INPUT_FIXTURE_RESULT")?;
    let _runtime = CaptureRuntime::new()?;
    let mut desktop = Desktop::new()?;
    let helper = unsafe {
        FindWindowW(
            None,
            windows::core::w!("PaneFerry noninteractive helper verification"),
        )?
    };
    let mut helper_pid = 0;
    unsafe {
        GetWindowThreadProcessId(helper, Some(&mut helper_pid));
    }
    ensure!(
        helper_pid == pid,
        "expected a visible helper belonging to this fixture"
    );
    let windows = desktop.windows()?;
    ensure!(
        !windows
            .iter()
            .any(|w| w.pid == pid && w.name == "PaneFerry noninteractive helper verification"),
        "noninteractive fixture helper was offered for publishing"
    );
    let target = windows
        .into_iter()
        .find(|w| w.pid == pid && w.name == "PaneFerry input verification")
        .context("the dedicated input fixture must be visible")?;
    let window = HWND(target.handle as *mut _);
    let mut child_rect = RECT::default();
    let mut frame_rect = RECT::default();
    unsafe {
        GetWindowRect(GetWindow(window, GW_CHILD)?, &mut child_rect)?;
        DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut frame_rect as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )?;
    }
    let x = (child_rect.left + child_rect.right) / 2 - frame_rect.left;
    let y = (child_rect.top + child_rect.bottom) / 2 - frame_rect.top;
    if let Ok(blocker_pid) = std::env::var("PANEFERRY_INPUT_BLOCKER_PID") {
        let blocker = unsafe {
            FindWindowW(
                None,
                windows::core::w!("PaneFerry foreground blocker verification"),
            )?
        };
        // The dedicated blocker is a separate fixture process. Activate it for
        // setup; its activation handler locks programmatic foreground changes.
        let blocker_pid: u32 = blocker_pid.parse()?;
        let mut actual_pid = 0;
        unsafe {
            GetWindowThreadProcessId(blocker, Some(&mut actual_pid));
        }
        ensure!(actual_pid == blocker_pid, "unexpected blocker process");
        unsafe {
            let _ = SetForegroundWindow(blocker);
        }
        std::thread::sleep(Duration::from_millis(200));
        ensure!(
            unsafe { GetForegroundWindow() } == blocker,
            "cover fixture must be foreground"
        );
        ensure!(
            unsafe {
                GetAncestor(
                    WindowFromPoint(POINT {
                        x: frame_rect.left + x,
                        y: frame_rect.top + y,
                    }),
                    GA_ROOT,
                )
            } == blocker,
            "fixture must be covered at the click point"
        );
        ensure!(
            !unsafe { SetForegroundWindow(window) }.as_bool(),
            "cover fixture must deny direct programmatic activation"
        );
    }
    ensure!(
        unsafe { GetForegroundWindow() } != window,
        "fixture must start in background"
    );
    let input_result = (|| -> Result<()> {
        if std::env::var_os("PANEFERRY_INPUT_FOCUS_FIRST").is_some() {
            let mut before = POINT::default();
            unsafe {
                GetCursorPos(&mut before)?;
            }
            desktop.input(&target, Input::Focus)?;
            let deadline = Instant::now() + Duration::from_secs(2);
            while unsafe { GetForegroundWindow() } != window && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            ensure!(
                unsafe { GetForegroundWindow() } == window,
                "explicit remote focus did not activate the covered fixture"
            );
            let mut after = POINT::default();
            unsafe {
                GetCursorPos(&mut after)?;
            }
            ensure!(before == after, "remote focus moved the local pointer");
        }
        desktop.input(
            &target,
            Input::Pointer {
                x,
                y,
                button: 1,
                pressed: true,
            },
        )?;
        desktop.input(
            &target,
            Input::Pointer {
                x,
                y,
                button: 1,
                pressed: false,
            },
        )?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while unsafe { GetForegroundWindow() } != window && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        ensure!(
            unsafe { GetForegroundWindow() } == window,
            "click did not activate fixture"
        );
        let mut gui = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        let child = unsafe { GetWindow(window, GW_CHILD)? };
        loop {
            unsafe {
                GetGUIThreadInfo(GetWindowThreadProcessId(window, None), &mut gui)?;
            }
            if gui.hwndFocus == child || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        ensure!(
            gui.hwndFocus == child,
            "click did not focus the fixture input box"
        );
        let text = "PaneFerry input test 中文🙂";
        desktop.input(&target, Input::Text(text.into()))?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if std::fs::read_to_string(&result_path).ok().as_deref() == Some(text) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        anyhow::bail!("fixture did not receive the expected Unicode text")
    })();
    desktop.detach(); // Also release a down button if an earlier assertion failed.
    input_result
}
