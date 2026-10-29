mod capture;
mod video;
use crate::{Geometry, Input, Target, VideoFrame, WindowSystem};
use anyhow::{bail, ensure, Result};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashSet, VecDeque},
    mem::size_of,
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
    capture: Option<capture::Capture>,
    encoder: Option<video::Encoder>,
    pending: VecDeque<VideoFrame>,
    keys: HashSet<u16>,
    buttons: HashSet<u8>,
    fps: u32,
    stream: u64,
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

impl Desktop {
    pub fn new() -> Result<Self> {
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)?;
            if let Err(error) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                RoUninitialize();
                return Err(error.into());
            }
            // This worker uses physical capture pixels, including on mixed-DPI monitors.
            let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        Ok(Self {
            capture: None,
            encoder: None,
            pending: VecDeque::new(),
            keys: HashSet::new(),
            buttons: HashSet::new(),
            fps: 30,
            stream: 0,
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
    fn focus(&self, target: &Target) -> Result<()> {
        validate(target)?;
        unsafe {
            if IsIconic(hwnd(target)).as_bool() {
                let _ = ShowWindow(hwnd(target), SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd(target));
            if GetForegroundWindow() != hwnd(target) {
                let foreground = GetForegroundWindow();
                let thread = GetWindowThreadProcessId(foreground, None);
                let current = GetCurrentThreadId();
                let mut message = MSG::default();
                let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
                if thread != 0
                    && thread != current
                    && AttachThreadInput(current, thread, true).as_bool()
                {
                    let _ = SetForegroundWindow(hwnd(target));
                    let _ = AttachThreadInput(current, thread, false);
                }
            }
            ensure!(
                GetForegroundWindow() == hwnd(target),
                "input_focus_required"
            );
        }
        Ok(())
    }
}
unsafe extern "system" fn collect(window: HWND, data: LPARAM) -> BOOL {
    let targets = &mut *(data.0 as *mut Vec<Target>);
    if targets.len() >= 128
        || !IsWindowVisible(window).as_bool()
        || IsIconic(window).as_bool()
        || !GetPropW(window, windows::core::w!("PaneFerry.RemoteWindow.1"))
            .0
            .is_null()
        || GetWindow(window, GW_OWNER).is_ok_and(|h| !h.0.is_null())
    {
        return BOOL(1);
    }
    let mut cloaked = 0u32;
    if DwmGetWindowAttribute(window, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4).is_err()
        || cloaked != 0
    {
        return BOOL(1);
    }
    let name = title(window);
    if name.is_empty() {
        return BOOL(1);
    }
    let mut pid = 0;
    GetWindowThreadProcessId(window, Some(&mut pid));
    if pid == GetCurrentProcessId() || !paneferry_platform::process_is_current_user(pid) {
        return BOOL(1);
    }
    let Ok(start) = started(pid) else {
        return BOOL(1);
    };
    // Exclude this product's helpers by process image, never by window title.
    if let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
        let mut path = [0u16; 32768];
        let mut length = path.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &mut length,
        );
        let _ = CloseHandle(process);
        if result.is_err() {
            return BOOL(1);
        }
        let path = String::from_utf16_lossy(&path[..length as usize]).to_lowercase();
        let filename = path.rsplit('\\').next().unwrap_or("");
        if filename.starts_with("paneferry") || filename == "lan-remoteapp-launcher.exe" {
            return BOOL(1);
        }
    } else {
        return BOOL(1);
    }
    // Window properties disappear on destruction, unlike a recycled HWND/PID.
    // SetProp also rejects elevated windows that this standard-user publisher cannot control.
    let mut generation = GetPropW(window, WINDOW_IDENTITY).0 as usize;
    if generation == 0 {
        generation = (uuid::Uuid::new_v4().as_u128() as usize).max(1);
        if SetPropW(window, WINDOW_IDENTITY, Some(HANDLE(generation as *mut _))).is_err() {
            return BOOL(1);
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
    };
    if validate(&target).is_ok()
        && bounds(&target).is_ok_and(|r| {
            paneferry_protocol::dimensions((r.right - r.left) as u32, (r.bottom - r.top) as u32)
                .is_ok()
        })
    {
        targets.push(target);
    }
    BOOL(1)
}

impl WindowSystem for Desktop {
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
        validate(target)?;
        let rect = bounds(target)?;
        let (width, height) = (
            (rect.right - rect.left) as u32,
            (rect.bottom - rect.top) as u32,
        );
        paneferry_protocol::dimensions(width, height)?;
        Ok(Geometry {
            title: title(hwnd(target)),
            width,
            height,
            visible: unsafe {
                IsWindowVisible(hwnd(target)).as_bool() && !IsIconic(hwnd(target)).as_bool()
            },
        })
    }
    fn attach(&mut self, target: &Target, fps: u32) -> Result<()> {
        validate(target)?;
        self.detach();
        self.fps = fps.clamp(1, 30);
        self.capture = Some(capture::Capture::new(hwnd(target))?);
        Ok(())
    }
    fn keyframe(&mut self, target: &Target) -> Result<()> {
        validate(target)?;
        self.encoder = None;
        self.pending.clear();
        self.capture = Some(capture::Capture::new(hwnd(target))?);
        Ok(())
    }
    fn frame(&mut self, target: &Target) -> Result<Option<VideoFrame>> {
        validate(target)?;
        let geometry = self.geometry(target)?;
        if !geometry.visible {
            self.release();
            self.capture = None;
            self.encoder = None;
            self.pending.clear();
            return Ok(None);
        }
        if let Some(frame) = self.pending.pop_front() {
            if frame.width == geometry.width && frame.height == geometry.height {
                return Ok(Some(frame));
            }
            self.pending.clear();
            self.encoder = None;
        }
        if self.capture.is_none() {
            self.capture = Some(capture::Capture::new(hwnd(target))?);
        }
        let Some((width, height, pixels)) = self.capture.as_mut().unwrap().frame()? else {
            return Ok(None);
        };
        if width != geometry.width || height != geometry.height {
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
            self.encoder = Some(video::Encoder::new(w, h, self.fps, self.stream)?);
        }
        let encoder = self.encoder.as_mut().unwrap();
        for (data, key_frame) in encoder.encode(&crate::pixels::nv12(&pixels, width, height)?)? {
            self.pending.push_back(VideoFrame {
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
        if !releasing {
            if !matches!(input, Input::Focus) {
                ensure!(self.geometry(target)?.visible, "window_unavailable");
            }
            self.focus(target)?;
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
                    ensure!(
                        unsafe { GetForegroundWindow() } == hwnd(target),
                        "input_focus_required"
                    );
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
                    let under = WindowFromPoint(POINT {
                        x: rect.left + x,
                        y: rect.top + y,
                    });
                    ensure!(
                        GetAncestor(under, GA_ROOT) == hwnd(target),
                        "input_focus_required"
                    );
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
                self.capture = None;
                self.encoder = None;
                self.pending.clear();
            }
            Input::Close => unsafe {
                PostMessageW(Some(hwnd(target)), WM_CLOSE, WPARAM(0), LPARAM(0))?;
            },
        }
        Ok(())
    }
    fn detach(&mut self) {
        self.release();
        self.capture = None;
        self.encoder = None;
        self.pending.clear();
    }
}
impl Drop for Desktop {
    fn drop(&mut self) {
        self.detach();
        unsafe {
            let _ = MFShutdown();
            RoUninitialize();
        }
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
