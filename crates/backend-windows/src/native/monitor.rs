//! Explicit, per-monitor desktop sharing on the current interactive desktop.
use super::*;
use windows::Win32::Graphics::Gdi::*;

fn info(handle: HMONITOR) -> Result<MONITORINFOEXW> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    ensure!(
        unsafe { GetMonitorInfoW(handle, &mut info.monitorInfo) }.as_bool(),
        "desktop_monitor_unavailable"
    );
    let r = info.monitorInfo.rcMonitor;
    // Bound raw capture allocation independently of the encoded canvas.
    ensure!(
        r.right > r.left && r.bottom > r.top,
        "desktop_monitor_unavailable"
    );
    let (w, h) = ((r.right - r.left) as u64, (r.bottom - r.top) as u64);
    ensure!(
        w <= 8192 && h <= 8192 && w * h <= 32_000_000,
        "desktop_monitor_unavailable"
    );
    Ok(info)
}
fn id(info: &MONITORINFOEXW) -> String {
    let end = info
        .szDevice
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(info.szDevice.len());
    format!(
        "monitor-{:x}",
        Sha256::digest(String::from_utf16_lossy(&info.szDevice[..end]).as_bytes())
    )
}
unsafe extern "system" fn collect(handle: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let monitors = &mut *(data.0 as *mut Vec<(crate::Monitor, Target)>);
    if monitors.len() >= 16 {
        return BOOL(1);
    }
    if let Ok(info) = info(handle) {
        let id = id(&info);
        let r = info.monitorInfo.rcMonitor;
        let primary = info.monitorInfo.dwFlags & 1 != 0;
        let name = format!(
            "Windows 显示器 {}{}",
            monitors.len() + 1,
            if primary { " · 主屏幕" } else { "" }
        );
        monitors.push((
            crate::Monitor {
                id: id.clone(),
                name: name.clone(),
                width: (r.right - r.left) as u32,
                height: (r.bottom - r.top) as u32,
                primary,
            },
            Target {
                monitor: Some(id.clone()),
                id: id.clone(),
                name,
                handle: handle.0 as usize,
                pid: 0,
                started: 0,
                generation: 0,
                application_key: id,
                auto_connectable: false,
            },
        ));
    }
    BOOL(1)
}
pub fn list() -> Result<Vec<(crate::Monitor, Target)>> {
    desktop_ready().map_err(|_| anyhow::anyhow!("desktop_locked"))?;
    let mut values = Vec::new();
    ensure!(
        unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(collect),
                LPARAM((&mut values as *mut Vec<(crate::Monitor, Target)>) as isize),
            )
        }
        .as_bool(),
        "desktop_monitor_unavailable"
    );
    Ok(values)
}
pub fn bounds(target: &Target) -> Result<RECT> {
    let info =
        info(HMONITOR(target.handle as *mut _)).map_err(|_| anyhow::anyhow!("session_exited"))?;
    ensure!(
        target.monitor.as_deref() == Some(id(&info).as_str()),
        "session_exited"
    );
    Ok(info.monitorInfo.rcMonitor)
}

pub fn canvas(rect: RECT) -> (u32, u32) {
    let (w, h) = scene::sized(rect);
    // Avoid exceeding the atlas budget when a large monitor is scaled down
    // and its odd canvas edges would otherwise be padded back up.
    ((w & !1).max(2), (h & !1).max(2))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_and_negative_origin_monitors_fit_every_color_mode() {
        for (w, h) in [(3840, 2160), (8192, 2160), (2160, 3840), (2560, 1440)] {
            let (a, b) = canvas(RECT {
                left: -w,
                top: -h,
                right: 0,
                bottom: 0,
            });
            for mode in [
                crate::color::ColorMode::Compatible,
                crate::color::ColorMode::Full,
                crate::color::ColorMode::FullAligned,
            ] {
                mode.dimensions(a, b).unwrap();
            }
            assert!(a <= w as u32 && b <= h as u32);
        }
    }
    #[test]
    #[ignore = "reads the current display; no input injection or saved screen pixels"]
    fn monitor_capture_routes_pixels_and_preserves_desktop() -> Result<()> {
        let mut desktop = Desktop::new()?;
        let monitors = desktop.monitors()?;
        let monitor = monitors
            .iter()
            .find(|m| m.primary)
            .context("no primary monitor")?;
        let target = desktop.monitor_target(&monitor.id)?;
        ensure!(
            desktop.monitor_target("not-a-monitor").is_err(),
            "unknown monitor accepted"
        );
        let mut forged = target.clone();
        forged.monitor = Some("not-a-monitor".into());
        ensure!(
            validate(&forged).is_err(),
            "changed monitor identity accepted"
        );
        desktop.set_video_color(crate::color::ColorMode::FullAligned)?;
        desktop.attach(&target, 60)?;
        let g = desktop.geometry(&target)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut count = 0;
        while count < 8 && std::time::Instant::now() < deadline {
            if let Some(frame) = desktop.frame(&target)? {
                ensure!(
                    frame.width == g.width && frame.height == g.height && !frame.data.is_empty(),
                    "bad monitor frame"
                );
                if count == 0 {
                    ensure!(frame.key_frame, "missing initial IDR");
                }
                count += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        ensure!(count == 8, "monitor capture stalled");
        let scene = desktop.scene.as_ref().unwrap().input_snapshot();
        let r = bounds(&target)?;
        let (routed, pointer) = scene.route(Input::Pointer {
            x: 0,
            y: 0,
            button: 0,
            pressed: false,
        })?;
        ensure!(
            routed.monitor == target.monitor
                && pointer
                    == Input::Pointer {
                        x: 0,
                        y: 0,
                        button: 0,
                        pressed: false
                    },
            "origin routing"
        );
        let (_, pointer) = scene.route(Input::Pointer {
            x: g.width as i32 - 1,
            y: g.height as i32 - 1,
            button: 0,
            pressed: false,
        })?;
        let Input::Pointer { x, y, .. } = pointer else {
            unreachable!()
        };
        ensure!(
            x >= 0 && y >= 0 && x < r.right - r.left && y < r.bottom - r.top,
            "scaled edge routing"
        );
        ensure!(
            scene
                .route(Input::Pointer {
                    x: g.width as i32,
                    y: 0,
                    button: 1,
                    pressed: true
                })
                .is_err(),
            "outside input accepted"
        );
        ensure!(
            desktop
                .input(&target, Input::Close)
                .unwrap_err()
                .root_cause()
                .to_string()
                == "desktop_close_unavailable",
            "desktop close"
        );
        ensure!(
            desktop
                .input(
                    &target,
                    Input::Resize {
                        width: 640,
                        height: 480
                    }
                )
                .unwrap_err()
                .root_cause()
                .to_string()
                == "desktop_geometry_fixed",
            "desktop resize"
        );
        desktop.detach();
        validate(&target)?;
        eprintln!(
            "MONITOR_CAPTURE frames={count} source={}x{} canvas={}x{}",
            monitor.width, monitor.height, g.width, g.height
        );
        Ok(())
    }
}
