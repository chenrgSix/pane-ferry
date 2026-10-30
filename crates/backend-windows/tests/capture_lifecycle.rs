//! Opt-in real desktop check. Captures an eligible visible window in memory;
//! does not save pixels, send input, close the source or modify its contents.
#![cfg(windows)]

use anyhow::{ensure, Context, Result};
use paneferry_backend_windows::{CaptureRuntime, Desktop, Input, WindowSystem};
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires an unlocked interactive Windows desktop with a visible normal application"]
fn reconnect_capture_workers_with_service_runtime_alive() -> Result<()> {
    let _runtime = CaptureRuntime::new().context("initialize service runtime")?;
    let selected = std::env::var("PANEFERRY_CAPTURE_WINDOW_ID").ok();
    for cycle in 0..10 {
        // Match publisher workers: every connection constructs and drops its
        // own Desktop on a different thread while the service runtime stays up.
        let selected = selected.clone();
        std::thread::spawn(move || -> Result<()> {
            let mut desktop = Desktop::new().context("initialize worker runtime")?;
            let target = desktop
                .windows()
                .context("enumerate test windows")?
                .into_iter()
                .find(|w| {
                    selected
                        .as_ref()
                        .map_or(w.auto_connectable, |id| &w.id == id)
                })
                .ok_or_else(|| anyhow::anyhow!("no eligible test window"))?;
            let geometry = desktop.geometry(&target)?;
            desktop.attach(&target, 30).context("start WGC capture")?;
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut frames = 0;
            while Instant::now() < deadline && frames < 2 {
                if let Some(frame) = desktop.frame(&target).with_context(|| {
                    format!(
                        "capture and encode {}x{} frame",
                        geometry.width, geometry.height
                    )
                })? {
                    ensure!(!frame.data.is_empty(), "empty H.264 frame");
                    if frames == 0 {
                        ensure!(frame.key_frame, "missing initial keyframe");
                    }
                    frames += 1;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            ensure!(
                frames >= 2,
                "cycle {cycle}: no usable captured H.264 frames"
            );
            desktop.detach();
            Ok(())
        })
        .join()
        .expect("capture worker panicked")?;
    }
    Ok(())
}

#[test]
#[ignore = "requires an unlocked desktop with a visible background application; do not move the local mouse during the test"]
fn passive_pointer_does_not_activate_or_report_errors_for_background_window() -> Result<()> {
    use windows::Win32::{
        Foundation::{HWND, POINT},
        UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow},
    };
    let _runtime = CaptureRuntime::new()?;
    let mut desktop = Desktop::new()?;
    let foreground = unsafe { GetForegroundWindow() };
    let target = desktop
        .windows()?
        .into_iter()
        .find(|w| w.auto_connectable && HWND(w.handle as *mut _) != foreground)
        .ok_or_else(|| anyhow::anyhow!("no eligible background test window"))?;
    let mut cursor_before = POINT::default();
    unsafe {
        GetCursorPos(&mut cursor_before)?;
    }
    for _ in 0..100 {
        desktop.input(
            &target,
            Input::Pointer {
                x: 1,
                y: 1,
                button: 0,
                pressed: false,
            },
        )?;
    }
    let mut cursor_after = POINT::default();
    unsafe {
        GetCursorPos(&mut cursor_after)?;
    }
    ensure!(
        unsafe { GetForegroundWindow() } == foreground,
        "hover stole foreground focus"
    );
    ensure!(
        cursor_before.x == cursor_after.x && cursor_before.y == cursor_after.y,
        "background hover moved the local pointer"
    );
    Ok(())
}

#[test]
#[ignore = "profiles a visible desktop window without saving pixels or sending input"]
fn capture_pipeline_throughput_probe() -> Result<()> {
    let _runtime = CaptureRuntime::new()?;
    let mut desktop = Desktop::new()?;
    let selected = std::env::var("PANEFERRY_CAPTURE_WINDOW_ID").ok();
    let target = desktop
        .windows()?
        .into_iter()
        .find(|w| {
            selected
                .as_ref()
                .map_or(w.auto_connectable, |id| &w.id == id)
        })
        .context("no eligible window")?;
    desktop.attach(&target, 60)?;
    let geometry = desktop.geometry(&target)?;
    let mut timings = Vec::new();
    let mut bytes = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    while timings.len() < 120 && Instant::now() < deadline {
        let start = Instant::now();
        desktop.geometry(&target)?;
        if let Some(frame) = desktop.frame(&target)? {
            timings.push(start.elapsed().as_secs_f64() * 1000.0);
            bytes += frame.data.len();
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    ensure!(!timings.is_empty(), "no captured frame");
    timings.sort_by(f64::total_cmp);
    println!(
        "PIPELINE {}x{} frames={} bytes={} p50_ms={:.2} p95_ms={:.2}",
        geometry.width,
        geometry.height,
        timings.len(),
        bytes,
        timings[timings.len() / 2],
        timings[timings.len() * 95 / 100]
    );
    desktop.detach();
    Ok(())
}
