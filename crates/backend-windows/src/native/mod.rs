mod applications;
mod capture;
mod scene;
mod video;
use crate::{Geometry, Input, Target, VideoFrame, WindowSystem};
use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashSet, VecDeque},
    marker::PhantomData,
    mem::size_of,
    rc::Rc,
};
use windows::{
    core::{BOOL, PWSTR},
    Win32::{
        Foundation::*,
        Graphics::Dwm::*,
        Media::MediaFoundation::*,
        System::{
            RemoteDesktop::ProcessIdToSessionId, StationsAndDesktops::*, Threading::*, WinRT::*,
        },
        UI::{HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
};

pub struct Desktop {
    scene: Option<scene::Scene>,
    encoder: Option<video::Encoder>,
    pending: VecDeque<VideoFrame>,
    keys: HashSet<u16>,
    buttons: HashSet<u8>,
    fps: u32,
    stream: u64,
    // Dropped after all capture/encoder COM objects, on their owning thread.
    _runtime: CaptureRuntime,
}

/// Keep one on the publisher's main thread for its full service lifetime.
/// Short-lived discovery connections must not tear down the last MTA while
/// GraphicsCapture's internal threads are still completing asynchronous work.
pub struct CaptureRuntime {
    _thread_bound: PhantomData<Rc<()>>,
}
impl CaptureRuntime {
    pub fn new() -> Result<Self> {
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)?;
            if let Err(error) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                RoUninitialize();
                return Err(error.into());
            }
        }
        Ok(Self {
            _thread_bound: PhantomData,
        })
    }
}
impl Drop for CaptureRuntime {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            RoUninitialize();
        }
    }
}
const WINDOW_IDENTITY: windows::core::PCWSTR = windows::core::w!("PaneFerry.WindowIdentity.1");
fn hwnd(target: &Target) -> HWND {
    HWND(target.handle as *mut _)
}
fn started(pid: u32) -> Result<u64> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?;
        let mut start = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let result = GetProcessTimes(process, &mut start, &mut exit, &mut kernel, &mut user);
        let _ = CloseHandle(process);
        result?;
        Ok(((start.dwHighDateTime as u64) << 32) | start.dwLowDateTime as u64)
    }
}
fn desktop_ready() -> Result<()> {
    unsafe {
        let desktop = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS)?;
        let mut buffer = [0u16; 128];
        let mut needed = 0;
        let result = GetUserObjectInformationW(
            HANDLE(desktop.0),
            UOI_NAME,
            Some(buffer.as_mut_ptr().cast()),
            size_of::<[u16; 128]>() as u32,
            Some(&mut needed),
        );
        let _ = CloseDesktop(desktop);
        result?;
        let name = String::from_utf16_lossy(
            &buffer[..buffer.iter().position(|v| *v == 0).unwrap_or(buffer.len())],
        );
        ensure!(name.eq_ignore_ascii_case("default"), "desktop_locked");
        Ok(())
    }
}
fn validate(target: &Target) -> Result<()> {
    desktop_ready().map_err(|_| anyhow::anyhow!("desktop_locked"))?;
    unsafe {
        ensure!(IsWindow(Some(hwnd(target))).as_bool(), "session_exited");
        ensure!(
            GetPropW(hwnd(target), WINDOW_IDENTITY).0 as usize == target.generation,
            "session_exited"
        );
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd(target), Some(&mut pid));
        ensure!(
            pid == target.pid && started(pid).ok() == Some(target.started),
            "session_exited"
        );
        ensure!(
            paneferry_platform::process_is_current_user(pid),
            "input_unavailable"
        );
        let mut session = 0;
        let mut own_session = 0;
        ProcessIdToSessionId(pid, &mut session)?;
        ProcessIdToSessionId(GetCurrentProcessId(), &mut own_session)?;
        ensure!(session == own_session, "input_unavailable");
        if target.application_key.starts_with("appx-") {
            // A shared application host can retain its HWND while changing the
            // packaged application it represents. Keep the approved identity.
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?;
            let key = applications::packaged_window_key(hwnd(target), process);
            let _ = CloseHandle(process);
            ensure!(
                key.as_deref() == Some(target.application_key.as_str()),
                "session_exited"
            );
        }
        Ok(())
    }
}
fn bounds(target: &Target) -> Result<RECT> {
    unsafe {
        let mut rect = RECT::default();
        DwmGetWindowAttribute(
            hwnd(target),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut rect as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        )?;
        ensure!(
            rect.right > rect.left && rect.bottom > rect.top,
            "window_unavailable"
        );
        Ok(rect)
    }
}
fn title(window: HWND) -> String {
    let mut buffer = [0u16; 513];
    let length = unsafe { GetWindowTextW(window, &mut buffer) }.max(0) as usize;
    String::from_utf16_lossy(&buffer[..length])
        .chars()
        .filter(|c| !c.is_control())
        .take(256)
        .collect()
}

fn window_geometry(target: &Target) -> Result<Geometry> {
    validate(target)?;
    let rect = bounds(target)?;
    let (width, height) = (
        (rect.right - rect.left) as u32,
        (rect.bottom - rect.top) as u32,
    );
    paneferry_protocol::dimensions(width, height)?;
    let current = title(hwnd(target));
    Ok(Geometry {
        content_revision: 0,
        title: if current.is_empty() {
            target.name.clone()
        } else {
            current
        },
        width,
        height,
        visible: unsafe {
            IsWindowVisible(hwnd(target)).as_bool() && !IsIconic(hwnd(target)).as_bool()
        },
    })
}

impl Desktop {
    fn input_foreground(&self, target: &Target) -> bool {
        self.scene.as_ref().map_or_else(
            || unsafe { GetForegroundWindow() == hwnd(target) },
            |s| s.foreground_for(target),
        )
    }
    fn application_window(&mut self, id: &str) -> Result<Option<Target>> {
        let mut windows = self
            .windows()?
            .into_iter()
            .filter(|w| w.application_key == id);
        let first = windows.next();
        ensure!(windows.next().is_none(), "application_multiple_windows");
        Ok(first)
    }
    pub fn new() -> Result<Self> {
        let runtime = CaptureRuntime::new()?;
        unsafe {
            // This worker uses physical capture pixels, including on mixed-DPI monitors.
            let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        Ok(Self {
            scene: None,
            encoder: None,
            pending: VecDeque::new(),
            keys: HashSet::new(),
            buttons: HashSet::new(),
            fps: 30,
            stream: 0,
            _runtime: runtime,
        })
    }
    fn release(&mut self) {
        let keys: Vec<_> = self.keys.drain().collect();
        for key in keys {
            let _ = key_event(key, false);
        }
        let buttons: Vec<_> = self.buttons.drain().collect();
        for button in buttons {
            let _ = mouse_button(button, false);
        }
    }
    fn prepare_focus(&self, target: &Target) -> Result<()> {
        validate(target)?;
        unsafe {
            // A modal dialog may have disabled its owner. Do not repeatedly
            // activate that owner or deliver input to an unseen owned surface.
            if !IsWindowEnabled(hwnd(target)).as_bool() {
                return Err(anyhow::anyhow!("input_focus_required")
                    .context("target disabled by modal window"));
            }
            if self.input_foreground(target) {
                return Ok(());
            }
            if IsIconic(hwnd(target)).as_bool() {
                let _ = ShowWindow(hwnd(target), SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd(target));
            if GetForegroundWindow() != hwnd(target) {
                // Activation across input queues is asynchronous. Wait for its
                // notification without joining another application's input queue
                // (which could hang this worker when that application hangs).
                let _ = SendMessageTimeoutW(
                    hwnd(target),
                    WM_NULL,
                    WPARAM(0),
                    LPARAM(0),
                    SMTO_ABORTIFHUNG | SMTO_BLOCK,
                    200,
                    None,
                );
            }
            if GetForegroundWindow() != hwnd(target) {
                // The remote focus/click arrived over the network, not through
                // the Windows input queue. Submit a zero-distance mouse event
                // for this active operation before requesting foreground again.
                // It changes no pointer position, presses no keys/buttons and
                // never runs for passive hover. Z-order changes alone also need
                // foreground permission and cannot solve this case.
                send(&[INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dwFlags: MOUSEEVENTF_MOVE,
                            ..Default::default()
                        },
                    },
                }])?;
                let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
                while GetForegroundWindow() != hwnd(target) && std::time::Instant::now() < deadline
                {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    let _ = SetForegroundWindow(hwnd(target));
                }
            }
        }
        Ok(())
    }
    fn focus(&self, target: &Target) -> Result<()> {
        self.prepare_focus(target)?;
        if !self.input_foreground(target) {
            return Err(
                anyhow::anyhow!("input_focus_required").context("foreground activation denied")
            );
        }
        Ok(())
    }
    fn pointer_motion(&mut self, target: &Target, x: i32, y: i32) -> Result<()> {
        unsafe {
            // Hover is not a focus request. Receivers send it at pointer rate;
            // ignoring a background/covered window avoids focus fights and an
            // error/beep for every mouse movement. Release any interrupted drag.
            if !self.input_foreground(target)
                || !IsWindowEnabled(hwnd(target)).as_bool()
                || !window_geometry(target)?.visible
            {
                self.release();
                return Ok(());
            }
            let rect = bounds(target)?;
            if x < 0 || y < 0 || x >= rect.right - rect.left || y >= rect.bottom - rect.top {
                return Ok(());
            }
            let point = POINT {
                x: rect.left + x,
                y: rect.top + y,
            };
            if GetAncestor(WindowFromPoint(point), GA_ROOT) != hwnd(target) {
                self.release();
                return Ok(());
            }
            SetCursorPos(point.x, point.y)?;
        }
        Ok(())
    }
}
// Owned top-level dialogs are separate WGC surfaces. Child controls remain in
// their parent's capture; no-activate menus/tooltips and tray helpers do not.
unsafe fn visible_surface(window: HWND) -> bool {
    if !IsWindowVisible(window).as_bool()
        || IsIconic(window).as_bool()
        || window == GetShellWindow()
        || GetWindowLongPtrW(window, GWL_STYLE) as u32 & WS_CHILD.0 != 0
        || GetWindowLongPtrW(window, GWL_EXSTYLE) as u32 & WS_EX_NOACTIVATE.0 != 0
        || !GetPropW(window, windows::core::w!("PaneFerry.RemoteWindow.1"))
            .0
            .is_null()
    {
        return false;
    }
    let mut cloaked = 0u32;
    if DwmGetWindowAttribute(window, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4).is_err()
        || cloaked != 0
    {
        return false;
    }
    true
}

unsafe fn application_owner(window: HWND) -> Option<HWND> {
    let mut root = window;
    for _ in 0..16 {
        if !visible_surface(root) {
            return None;
        }
        let owner = GetWindow(root, GW_OWNER).unwrap_or_default();
        if owner.0.is_null() {
            return (GetWindowLongPtrW(root, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 == 0
                && !title(root).is_empty())
            .then_some(root);
        }
        root = owner;
    }
    None
}

unsafe extern "system" fn collect(window: HWND, data: LPARAM) -> BOOL {
    let targets = &mut *(data.0 as *mut Vec<Target>);
    if targets.len() >= 128 {
        return BOOL(1);
    }
    let Some(owner) = application_owner(window) else {
        return BOOL(1);
    };
    if owner != window {
        let mut rect = RECT::default();
        if GetWindowRect(window, &mut rect).is_err()
            || rect.right - rect.left < 64
            || rect.bottom - rect.top < 32
        {
            return BOOL(1);
        }
        let mut owner_pid = 0;
        GetWindowThreadProcessId(owner, Some(&mut owner_pid));
        if owner_pid == GetCurrentProcessId()
            || !paneferry_platform::process_is_current_user(owner_pid)
        {
            return BOOL(1);
        }
    }
    let mut name = title(window);
    if name.is_empty() && owner != window {
        name = format!("{} — 对话框", title(owner));
    }
    if name.is_empty() {
        return BOOL(1);
    }
    if let Some(target) = describe_window(window, name) {
        targets.push(target);
    }
    BOOL(1)
}

unsafe fn describe_window(window: HWND, name: String) -> Option<Target> {
    let mut pid = 0;
    GetWindowThreadProcessId(window, Some(&mut pid));
    if pid == GetCurrentProcessId() || !paneferry_platform::process_is_current_user(pid) {
        return None;
    }
    let Ok(start) = started(pid) else {
        return None;
    };
    // Exclude this product's helpers by process image, never by window title.
    let application_key =
        if let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            let mut path = [0u16; 32768];
            let mut length = path.len() as u32;
            let result = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(path.as_mut_ptr()),
                &mut length,
            );
            let packaged_key = applications::packaged_window_key(window, process);
            let _ = CloseHandle(process);
            if result.is_err() {
                return None;
            }
            let path = String::from_utf16_lossy(&path[..length as usize]).to_lowercase();
            let filename = path.rsplit('\\').next().unwrap_or("");
            if filename.starts_with("paneferry") || filename == "lan-remoteapp-launcher.exe" {
                return None;
            }
            if let Some(key) = packaged_key {
                key
            } else {
                crate::application_key(&path)
            }
        } else {
            return None;
        };
    // Normal application windows and their activatable owned dialogs are
    // eligible for the existing controller's new-window discovery policy.
    let auto_connectable = !scene::claimed_as_secondary(window);
    // Window properties disappear on destruction, unlike a recycled HWND/PID.
    // SetProp also rejects elevated windows that this standard-user publisher cannot control.
    let mut generation = GetPropW(window, WINDOW_IDENTITY).0 as usize;
    if generation == 0 {
        generation = (uuid::Uuid::new_v4().as_u128() as usize).max(1);
        if SetPropW(window, WINDOW_IDENTITY, Some(HANDLE(generation as *mut _))).is_err() {
            return None;
        }
    }
    let id = format!(
        "win-{:x}",
        Sha256::digest(format!("{}:{pid}:{start}:{generation}", window.0 as usize).as_bytes())
    );
    let target = Target {
        id,
        handle: window.0 as usize,
        pid,
        started: start,
        generation,
        name,
        application_key,
        auto_connectable,
    };
    if validate(&target).is_ok()
        && bounds(&target).is_ok_and(|r| {
            paneferry_protocol::dimensions((r.right - r.left) as u32, (r.bottom - r.top) as u32)
                .is_ok()
        })
    {
        return Some(target);
    }
    None
}

impl WindowSystem for Desktop {
    fn applications(&mut self) -> Result<Vec<crate::Application>> {
        desktop_ready()?;
        applications::list()
    }
    fn launch_application(&mut self, id: &str, start: bool) -> Result<Target> {
        desktop_ready()?;
        ensure!(
            applications::list()?
                .iter()
                .any(|a| a.id == id && a.unavailable_reason.is_none()),
            "app_not_approved"
        );
        if let Some(target) = self.application_window(id)? {
            return Ok(target);
        }
        if start {
            applications::launch(id)?;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            if let Some(target) = self.application_window(id)? {
                return Ok(target);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        bail!("application_window_pending")
    }
    fn windows(&mut self) -> Result<Vec<Target>> {
        desktop_ready().map_err(|_| anyhow::anyhow!("desktop_locked"))?;
        let mut result = Vec::new();
        unsafe {
            EnumWindows(
                Some(collect),
                LPARAM((&mut result as *mut Vec<Target>) as isize),
            )?;
        }
        Ok(result)
    }
    fn geometry(&mut self, target: &Target) -> Result<Geometry> {
        if let Some(scene) = self.scene.as_mut().filter(|s| s.anchor.id == target.id) {
            return scene.geometry();
        }
        window_geometry(target)
    }
    fn attach(&mut self, target: &Target, fps: u32) -> Result<()> {
        validate(target)?;
        self.detach();
        self.fps = fps.clamp(1, 30);
        self.scene = Some(scene::Scene::new(target)?);
        Ok(())
    }
    fn keyframe(&mut self, target: &Target) -> Result<()> {
        validate(target)?;
        self.encoder = None;
        self.pending.clear();
        Ok(())
    }
    fn frame(&mut self, target: &Target) -> Result<Option<VideoFrame>> {
        validate(target)?;
        let geometry = self.geometry(target)?;
        if !geometry.visible {
            self.release();
            self.encoder = None;
            self.pending.clear();
            return Ok(None);
        }
        if let Some(frame) = self.pending.pop_front() {
            if frame.width == geometry.width
                && frame.height == geometry.height
                && frame.content_revision == geometry.content_revision
            {
                return Ok(Some(frame));
            }
            self.pending.clear();
            self.encoder = None;
        }
        let Some((width, height, pixels)) =
            self.scene.as_mut().context("capture_failed")?.frame()?
        else {
            return Ok(None);
        };
        let content_revision = self.scene.as_ref().unwrap().revision;
        if width != geometry.width
            || height != geometry.height
            || content_revision != geometry.content_revision
        {
            self.encoder = None;
            return Ok(None);
        }
        let (w, h) = (width + width % 2, height + height % 2);
        if self
            .encoder
            .as_ref()
            .is_none_or(|e| e.width != w || e.height != h)
        {
            self.stream = self
                .stream
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("stream exhausted"))?;
            self.encoder = Some(
                video::Encoder::new(w, h, self.fps, self.stream)
                    .with_context(|| format!("initialize H.264 encoder {w}x{h}"))?,
            );
        }
        let encoder = self.encoder.as_mut().unwrap();
        for (data, key_frame) in encoder.encode(&crate::pixels::nv12(&pixels, width, height)?)? {
            self.pending.push_back(VideoFrame {
                content_revision,
                data,
                key_frame,
                width,
                height,
                stream_id: encoder.stream,
            });
        }
        Ok(self.pending.pop_front())
    }
    fn input(&mut self, target: &Target, input: Input) -> Result<()> {
        validate(target)?;
        // Releases are independent of a surface that may have just closed.
        match input {
            Input::Key {
                virtual_key,
                pressed: false,
            } => {
                if self.keys.remove(&virtual_key) {
                    key_event(virtual_key, false)?;
                }
                return Ok(());
            }
            Input::Pointer {
                button: 1..=3,
                pressed: false,
                ..
            } => {
                if let Input::Pointer { button, .. } = input {
                    if self.buttons.remove(&button) {
                        mouse_button(button, false)?;
                    }
                }
                return Ok(());
            }
            _ => {}
        }
        let routed = if let Some(scene) = self.scene.as_ref().filter(|s| s.anchor.id == target.id) {
            if matches!(input, Input::Resize { .. }) && scene.multiple() {
                return Ok(());
            }
            let passive = matches!(input, Input::Pointer { button: 0, .. });
            match scene.route(input) {
                Ok(routed) => routed,
                Err(_) if passive => {
                    self.release();
                    return Ok(());
                }
                Err(error) => return Err(error),
            }
        } else {
            (target.clone(), input)
        };
        self.input_one(&routed.0, routed.1)
    }
    fn detach(&mut self) {
        self.scene = None;
        self.release();
        self.encoder = None;
        self.pending.clear();
    }
}
impl Drop for Desktop {
    fn drop(&mut self) {
        self.detach();
    }
}
fn send(events: &[INPUT]) -> Result<()> {
    ensure!(
        unsafe { SendInput(events, size_of::<INPUT>() as i32) } == events.len() as u32,
        "input_unavailable"
    );
    Ok(())
}
fn key_event(key: u16, pressed: bool) -> Result<()> {
    let extended = matches!(key, 0x21..=0x28 | 0x2d | 0x2e | 0xa3 | 0xa5 | 0x5b | 0x5c);
    send(&[INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(key),
                dwFlags: if pressed {
                    KEYBD_EVENT_FLAGS(0)
                } else {
                    KEYEVENTF_KEYUP
                } | if extended {
                    KEYEVENTF_EXTENDEDKEY
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                ..Default::default()
            },
        },
    }])
}
fn mouse_button(button: u8, pressed: bool) -> Result<()> {
    let (flags, data) = match (button, pressed) {
        (1, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (1, false) => (MOUSEEVENTF_LEFTUP, 0),
        (2, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (2, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (3, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (3, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (4, true) => (MOUSEEVENTF_WHEEL, 120),
        (5, true) => (MOUSEEVENTF_WHEEL, (-120i32) as u32),
        (6, true) => (MOUSEEVENTF_HWHEEL, (-120i32) as u32),
        (7, true) => (MOUSEEVENTF_HWHEEL, 120),
        (4..=7, false) => return Ok(()),
        _ => bail!("invalid_request"),
    };
    send(&[INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: flags,
                mouseData: data,
                ..Default::default()
            },
        },
    }])
}

impl Desktop {
    fn input_one(&mut self, target: &Target, input: Input) -> Result<()> {
        validate(target)?;
        if let Input::Pointer {
            x, y, button: 0, ..
        } = input
        {
            return self.pointer_motion(target, x, y);
        }
        if matches!(input, Input::Focus) {
            // Focusing the receiver prepares its source for a subsequent click.
            // A denied programmatic activation is not a failed user click/key.
            return self.prepare_focus(target);
        }
        // Key/button release must not steal focus; all new input must target this window.
        let releasing = matches!(
            input,
            Input::Key { pressed: false, .. }
                | Input::Pointer {
                    pressed: false,
                    button: 1..=3,
                    ..
                }
        );
        if !releasing && !matches!(input, Input::Resize { .. } | Input::Close) {
            ensure!(window_geometry(target)?.visible, "window_unavailable");
            if matches!(
                input,
                Input::Pointer {
                    button: 1..=3,
                    pressed: true,
                    ..
                }
            ) {
                self.prepare_focus(target)?;
            } else {
                self.focus(target)?;
            }
        }
        match input {
            Input::Focus => {}
            Input::Key {
                virtual_key,
                pressed,
            } => {
                if !pressed && !self.keys.contains(&virtual_key) {
                    return Ok(());
                }
                key_event(virtual_key, pressed)?;
                if pressed {
                    self.keys.insert(virtual_key);
                } else {
                    self.keys.remove(&virtual_key);
                }
            }
            Input::Text(text) => {
                paneferry_protocol::validate_text(&text)?;
                self.release();
                // UTF-16 code units preserve surrogate pairs for emoji and supplementary CJK.
                let units: Vec<_> = text.encode_utf16().collect();
                let mut offset = 0;
                while offset < units.len() {
                    let mut end = (offset + 64).min(units.len());
                    if end < units.len() && (0xd800..=0xdbff).contains(&units[end - 1]) {
                        end -= 1;
                    }
                    let chunk = &units[offset..end];
                    offset = end;
                    ensure!(self.input_foreground(target), "input_focus_required");
                    let mut events = Vec::new();
                    for unit in chunk {
                        for up in [false, true] {
                            events.push(INPUT {
                                r#type: INPUT_KEYBOARD,
                                Anonymous: INPUT_0 {
                                    ki: KEYBDINPUT {
                                        wVk: VIRTUAL_KEY(0),
                                        wScan: *unit,
                                        dwFlags: KEYEVENTF_UNICODE
                                            | if up {
                                                KEYEVENTF_KEYUP
                                            } else {
                                                KEYBD_EVENT_FLAGS(0)
                                            },
                                        ..Default::default()
                                    },
                                },
                            });
                        }
                    }
                    send(&events)?;
                }
            }
            Input::Pointer {
                x,
                y,
                button,
                pressed,
            } => {
                if releasing {
                    if self.buttons.remove(&button) {
                        mouse_button(button, false)?;
                    }
                    return Ok(());
                }
                let rect = bounds(target)?;
                ensure!(
                    x >= 0 && y >= 0 && x < rect.right - rect.left && y < rect.bottom - rect.top,
                    "invalid_request"
                );
                unsafe {
                    ensure!(
                        IsWindowEnabled(hwnd(target)).as_bool(),
                        "input_focus_required"
                    );
                    let point = POINT {
                        x: rect.left + x,
                        y: rect.top + y,
                    };
                    // Foreground ownership may change before the other UI
                    // thread finishes raising its window. Wait for the actual
                    // hit-test result, not just a WM_NULL acknowledgement.
                    let deadline =
                        std::time::Instant::now() + std::time::Duration::from_millis(250);
                    let mut under = WindowFromPoint(point);
                    while GetAncestor(under, GA_ROOT) != hwnd(target)
                        && self.input_foreground(target)
                        && std::time::Instant::now() < deadline
                    {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        under = WindowFromPoint(point);
                    }
                    validate(target)?;
                    ensure!(bounds(target)? == rect, "window_unavailable");
                    if GetAncestor(under, GA_ROOT) != hwnd(target) {
                        let hit = GetAncestor(under, GA_ROOT);
                        return Err(anyhow::anyhow!("input_focus_required").context(format!(
                            "pointer target occluded: target={:?} foreground={:?} hit={hit:?} hit_topmost={}",
                            hwnd(target), GetForegroundWindow(),
                            GetWindowLongPtrW(hit, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0
                        )));
                    }
                    SetCursorPos(rect.left + x, rect.top + y)?;
                }
                if button != 0 {
                    mouse_button(button, pressed)?;
                    if button <= 3 && pressed {
                        self.buttons.insert(button);
                    }
                }
            }
            Input::Resize { width, height } => {
                paneferry_protocol::dimensions(width, height)?;
                unsafe {
                    let frame = bounds(target)?;
                    let mut outer = RECT::default();
                    GetWindowRect(hwnd(target), &mut outer)?;
                    SetWindowPos(
                        hwnd(target),
                        None,
                        0,
                        0,
                        width as i32 + (outer.right - outer.left) - (frame.right - frame.left),
                        height as i32 + (outer.bottom - outer.top) - (frame.bottom - frame.top),
                        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                    )?;
                }
                self.encoder = None;
                self.pending.clear();
            }
            Input::Close => unsafe {
                PostMessageW(Some(hwnd(target)), WM_CLOSE, WPARAM(0), LPARAM(0))?;
            },
        }
        Ok(())
    }
}
