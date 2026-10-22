use crate::{process_identity, ProcessIdentity};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use remoteapp_core::{App, SessionBus};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    os::unix::{fs::MetadataExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Deserialize)]
pub struct DesktopApp {
    pub id: String,
    desktop_id: String,
    pub name: String,
    pub description: String,
    pub icon_png: Option<String>,
    startup_class: String,
    pub executable: String,
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub snap: Option<String>,
    #[serde(default)]
    pub session_bus: SessionBus,
    pub unavailable_reason: Option<String>,
}
impl DesktopApp {
    pub fn public(&self) -> Value {
        json!({"id":self.id,"name":self.name,"description":self.description,"icon_png":self.icon_png,
            "source":"desktop","unavailable_reason":self.unavailable_reason})
    }
    pub fn app(&self) -> App {
        App {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            executable: self.executable.clone(),
            argv: self.argv.clone(),
            session_bus: self.session_bus,
            existing: None,
        }
    }
    pub fn resolve(&self) -> Result<Self> {
        let entry: Option<Self> = serde_json::from_slice(&helper(&["resolve", &self.desktop_id])?)?;
        let entry = entry.context("desktop application unavailable")?;
        entry.validate()?;
        if entry.id != self.id || entry.unavailable_reason.is_some() {
            bail!("desktop application unavailable")
        }
        Ok(entry)
    }
    fn validate(&self) -> Result<()> {
        if !remoteapp_protocol::identifier(&self.id)
            || !self.id.starts_with("desktop-")
            || self.name.is_empty()
            || self.name.len() > 1024
            || self.description.len() > 4096
            || self.desktop_id.len() > 1024
            || self.startup_class.len() > 1024
            || self.argv.len() > 64
            || self.argv.iter().any(|s| s.len() > 4096 || s.contains('\0'))
            || self.cwd.as_ref().is_some_and(|p| !p.is_absolute())
            || self.snap.as_ref().is_some_and(|s| {
                s.is_empty()
                    || s.len() > 64
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            })
            || self.snap.is_some() && self.session_bus != SessionBus::Isolated
        {
            bail!("invalid desktop application")
        }
        if self.unavailable_reason.is_none()
            && (!Path::new(&self.executable).is_absolute() || self.executable.contains('\0'))
        {
            bail!("invalid desktop executable")
        }
        if let Some(icon) = &self.icon_png {
            let bytes = STANDARD.decode(icon)?;
            if bytes.len() > 16 * 1024 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                bail!("invalid desktop icon")
            }
        }
        Ok(())
    }
}

fn helper(arguments: &[&str]) -> Result<Vec<u8>> {
    let mut child = Command::new("/usr/bin/python3")
        .args(["-I", "-c", include_str!("desktop_apps.py")])
        .args(arguments)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().context("desktop discovery pipe")?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(12 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    let result = loop {
        if let Some(status) = child.try_wait()? {
            break status.success();
        }
        if Instant::now() >= deadline {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            break false;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let bytes = reader
        .join()
        .map_err(|_| anyhow::anyhow!("desktop discovery reader"))??;
    if !result || bytes.len() > 12 * 1024 * 1024 {
        bail!("application discovery unavailable")
    }
    Ok(bytes)
}
pub fn applications() -> Result<Vec<DesktopApp>> {
    let apps: Vec<DesktopApp> = serde_json::from_slice(&helper(&["list"])?)?;
    if apps.len() > 512 {
        bail!("application discovery limit")
    }
    let mut ids = std::collections::HashSet::new();
    for app in &apps {
        app.validate()?;
        if !ids.insert(&app.id) {
            bail!("duplicate desktop application")
        }
    }
    Ok(apps)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Desktop {
    pub display: u16,
    pub authority: PathBuf,
}
fn desktop(display: &str, authority: PathBuf) -> Option<Desktop> {
    let number = display.strip_prefix(':')?.split('.').next()?.parse().ok()?;
    let metadata = fs::metadata(&authority).ok()?;
    (metadata.is_file() && metadata.uid() == unsafe { libc::geteuid() }).then_some(Desktop {
        display: number,
        authority,
    })
}
pub fn environment() -> Option<Desktop> {
    let display = std::env::var("DISPLAY").ok()?;
    let authority = std::env::var_os("XAUTHORITY")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".Xauthority")))?;
    desktop(&display, authority)
}
pub fn desktops(explicit: &Option<Desktop>, configured: &[App]) -> Vec<Desktop> {
    if let Some(value) = explicit {
        if value.authority.exists() {
            return vec![value.clone()];
        }
    }
    let mut values = Vec::new();
    // Ask the user manager for exactly the graphical login's display/cookie.
    // Never enumerate /proc environments or change global XAUTHORITY.
    if let Ok(output) = Command::new("/usr/bin/systemctl")
        .args(["--user", "show-environment"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    {
        if output.stdout.len() <= 1024 * 1024 {
            let text = String::from_utf8_lossy(&output.stdout);
            let fields: HashMap<_, _> = text.lines().filter_map(|s| s.split_once('=')).collect();
            if let (Some(display), Some(authority)) =
                (fields.get("DISPLAY"), fields.get("XAUTHORITY"))
            {
                if let Some(value) = desktop(display, PathBuf::from(authority)) {
                    values.push(value);
                }
            }
        }
    }
    for app in configured {
        if let Some(existing) = &app.existing {
            if let Some(value) = desktop(
                &format!(":{}", existing.display),
                PathBuf::from(&existing.xauthority),
            ) {
                if !values.contains(&value) {
                    values.push(value);
                }
            }
        }
    }
    values.truncate(8);
    values
}

pub struct RunningWindow {
    pub id: String,
    pub desktop: Desktop,
    pub identity: ProcessIdentity,
    pub window_id: u32,
    pub title: String,
    pub name: String,
    pub minimized: bool,
    pub icon_png: Option<String>,
}
impl RunningWindow {
    pub fn app(&self) -> App {
        App {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.title.clone(),
            executable: self.identity.executable.to_string_lossy().into_owned(),
            argv: vec![],
            session_bus: SessionBus::Isolated,
            existing: Some(remoteapp_core::ExistingWindow {
                display: self.desktop.display,
                xauthority: self.desktop.authority.to_string_lossy().into_owned(),
                process_id: self.identity.pid,
                scale_factor: 1,
            }),
        }
    }
    pub fn public(&self, controlled: bool, session_id: Option<String>) -> Value {
        json!({"id":self.id,"window_id":self.window_id,"name":self.name,"title":self.title,
            "icon_png":self.icon_png,"minimized":self.minimized,"controlled":controlled,"session_id":session_id})
    }
}
pub fn running(
    desktops: &[Desktop],
    apps: &[DesktopApp],
    excluded: &[u16],
) -> Result<(HashMap<String, RunningWindow>, bool)> {
    let mut values = HashMap::new();
    let mut connected = false;
    'sources: for desktop in desktops {
        if excluded.contains(&desktop.display) {
            continue;
        }
        let Ok(windows) =
            remoteapp_backend_x11::desktop_windows(desktop.display, &desktop.authority)
        else {
            continue;
        };
        connected = true;
        for window in windows {
            let Ok(identity) = process_identity(window.process_id) else {
                continue;
            };
            let receiver = crate::runtime()?
                .join("receivers")
                .join(format!("{}.json", identity.pid));
            if !receiver.is_symlink()
                && fs::metadata(&receiver).is_ok_and(|metadata| metadata.len() <= 1024)
                && fs::read(&receiver)
                    .ok()
                    .and_then(|data| serde_json::from_slice::<Value>(&data).ok())
                    .is_some_and(|value| {
                        value["pid"] == identity.pid && value["started"] == identity.started
                    })
            {
                continue;
            }
            let matching = apps
                .iter()
                .find(|app| {
                    !window.class.is_empty()
                        && (app.startup_class.eq_ignore_ascii_case(&window.class)
                            || app
                                .desktop_id
                                .trim_end_matches(".desktop")
                                .eq_ignore_ascii_case(&window.class))
                })
                .or_else(|| {
                    apps.iter().find(|app| {
                        Path::new(&app.executable).file_name() == identity.executable.file_name()
                    })
                });
            // Stable across refresh or a transient discovery failure, but PID
            // reuse changes starttime and cannot retarget a stale launch ID.
            let id = format!(
                "window-{}-{}-{}-{}",
                desktop.display, identity.pid, identity.started, window.window_id
            );
            let icon_png = matching
                .and_then(|a| a.icon_png.clone())
                .or_else(|| window.icon_png.map(|bytes| STANDARD.encode(bytes)));
            let name = matching.map(|a| a.name.clone()).unwrap_or_else(|| {
                if window.class.is_empty() {
                    identity
                        .executable
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                } else {
                    window.class
                }
            });
            let value = RunningWindow {
                id: id.clone(),
                desktop: desktop.clone(),
                identity,
                window_id: window.window_id,
                title: window.title,
                name,
                minimized: window.minimized,
                icon_png,
            };
            values.insert(id, value);
            if values.len() >= 128 {
                break 'sources;
            }
        }
    }
    Ok((values, connected))
}
