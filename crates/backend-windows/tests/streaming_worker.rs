#![cfg(windows)]
use anyhow::{ensure, Context, Result};
use paneferry_backend_windows::{CaptureRuntime, Input, PublishingDesktop, WindowSystem};
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires windows-video.ps1 and PANEFERRY_VIDEO_FIXTURE_PID; only operates on that fixture"]
fn real_capture_worker_streams_and_accepts_input() -> Result<()> {
    let pid: u32 = std::env::var("PANEFERRY_VIDEO_FIXTURE_PID")?.parse()?;
    let _runtime = CaptureRuntime::new()?;
    let mut desktop = PublishingDesktop::new()?;
    let target = desktop
        .windows()?
        .into_iter()
        .find(|w| w.pid == pid && w.name == "PaneFerry dynamic video verification")
        .context("video fixture missing")?;
    if std::env::var("PANEFERRY_TEST_COLOR").as_deref() == Ok("444") {
        desktop.set_video_color(paneferry_backend_windows::color::ColorMode::Full)?;
    }
    desktop.attach(&target, 60)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut bitstream = Vec::new();
    let start = Instant::now();
    let mut frames = 0;
    let mut first_stream = None;
    let mut pending = desktop.begin_frame(&target)?;
    runtime.block_on(async {
        while frames < 120 && start.elapsed() < Duration::from_secs(15) {
            let frame = (&mut pending).await?;
            if let Some(frame) = frame {
                if first_stream.is_none() {
                    ensure!(frame.key_frame, "missing initial IDR");
                    first_stream = Some(frame.stream_id);
                }
                bitstream.extend_from_slice(&frame.data);
                frames += 1;
            }
            if frames == 0 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            let deadline = start + Duration::from_secs_f64(frames as f64 / 60.0);
            if deadline > Instant::now() {
                tokio::time::sleep_until(deadline.into()).await;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
            pending = desktop.begin_frame(&target)?;
            if frames == 30 {
                // The capture task is already outstanding when control executes.
                desktop.input(&target, Input::Focus)?;
                for pressed in [true, false] {
                    desktop.input(
                        &target,
                        Input::Pointer {
                            x: 80,
                            y: 100,
                            button: 1,
                            pressed,
                        },
                    )?;
                }
            }
        }
        Ok::<_, anyhow::Error>(())
    })?;
    ensure!(
        frames == 120,
        "capture worker stopped producing frames: {frames}"
    );
    if let Some(dir) = std::env::var_os("PANEFERRY_VIDEO_PROBE_DIR") {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            std::path::PathBuf::from(dir).join("fixture.h264"),
            bitstream,
        )?;
    }
    println!(
        "WORKER frames={frames} elapsed_ms={}",
        start.elapsed().as_millis()
    );
    drop(pending);
    desktop.detach();
    // A second worker must be able to acquire the released scene lease.
    desktop.attach(&target, 60)?;
    desktop.detach();
    Ok(())
}

#[test]
#[ignore = "requires windows-menus.ps1 and PANEFERRY_MENU_FIXTURE_PID/RESULT"]
fn capture_worker_routes_composed_menu_input() -> Result<()> {
    use windows::Win32::{
        Foundation::{HWND, RECT},
        Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
    };
    let pid: u32 = std::env::var("PANEFERRY_MENU_FIXTURE_PID")?.parse()?;
    let result = std::env::var("PANEFERRY_MENU_FIXTURE_RESULT")?;
    let _runtime = CaptureRuntime::new()?;
    let mut desktop = PublishingDesktop::new()?;
    let target = desktop
        .windows()?
        .into_iter()
        .find(|w| w.pid == pid && w.name == "PaneFerry menu verification")
        .context("menu fixture missing")?;
    desktop.attach(&target, 60)?;
    desktop.input(&target, Input::Focus)?;
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
    while Instant::now() < deadline && !std::path::Path::new(&(result.clone() + ".bounds")).exists()
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    let bounds: Vec<i32> = std::fs::read_to_string(result.clone() + ".bounds")?
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    // Let WGC discover and include the menu, then route using the published scene.
    for _ in 0..10 {
        let _ = desktop.frame(&target)?;
        std::thread::sleep(Duration::from_millis(16));
    }
    let mut anchor = RECT::default();
    unsafe {
        DwmGetWindowAttribute(
            HWND(target.handle as *mut _),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut anchor as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )?;
    }
    for pressed in [true, false] {
        desktop.input(
            &target,
            Input::Pointer {
                x: bounds[0] + 20 - anchor.left,
                y: bounds[1] + 12 - anchor.top,
                button: 1,
                pressed,
            },
        )?;
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline
        && std::fs::read_to_string(&result).unwrap_or_default() != "clicked"
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    ensure!(
        std::fs::read_to_string(&result)? == "clicked",
        "composed menu did not receive click"
    );
    desktop.detach();
    Ok(())
}
