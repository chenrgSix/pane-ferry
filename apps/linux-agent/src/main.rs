#[cfg(not(unix))]
compile_error!("Linux agent requires Unix");
use anyhow::{bail, Context, Result};
use paneferry_backend_x11::Backend;
use paneferry_core::{load_catalog, App, SessionBus};
use paneferry_protocol::{identifier, read_line_limit, Request, Window};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{BufReader, Read, Write},
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
        process::CommandExt,
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
static STOP: AtomicBool = AtomicBool::new(false);
static CLIENTS: AtomicUsize = AtomicUsize::new(0);
const ACTIVE_SCAN_INTERVAL: Duration = Duration::from_micros(16_667);
const IDLE_SCAN_INTERVAL: Duration = Duration::from_millis(100);

// Publisher policy is advertised to clients and enforced on every rate request.
// The protocol ceiling stays independent of an installation's chosen cap.
fn video_fps_limit(value: Option<&str>) -> Result<u8> {
    let Some(value) = value else {
        return Ok(paneferry_protocol::MAX_VIDEO_FPS);
    };
    let cap = value
        .parse::<u8>()
        .ok()
        .filter(|n| (1..=paneferry_protocol::MAX_VIDEO_FPS).contains(n));
    cap.context("PANEFERRY_VIDEO_MAX_FPS must be an integer between 1 and 90")
}

fn configured_video_fps_limit() -> Result<u8> {
    let value = std::env::var("PANEFERRY_VIDEO_MAX_FPS");
    match value {
        Ok(value) => video_fps_limit(Some(&value)),
        Err(std::env::VarError::NotPresent) => video_fps_limit(None),
        Err(error) => Err(error.into()),
    }
}

fn scan_interval(
    attached: bool,
    has_windows: bool,
    recent_window_action: bool,
    event_driven_metadata: bool,
) -> Duration {
    // Composed scenes place popups before paint. X11 structural notifications
    // independently wake discovery. Keep fast polling for other backends and
    // initial discovery, plus a bounded periodic liveness/metadata fallback.
    if !attached {
        Duration::from_secs(1)
    } else if !has_windows || (recent_window_action && !event_driven_metadata) {
        ACTIVE_SCAN_INTERVAL
    } else {
        IDLE_SCAN_INTERVAL
    }
}

fn triggers_fast_scan(request: &Request) -> bool {
    matches!(
        request,
        Request::Configure { .. }
            | Request::Focus { .. }
            | Request::Close { .. }
            | Request::Pointer {
                button: 1..=u8::MAX,
                pressed: true,
                ..
            }
            | Request::Key { pressed: true, .. }
    )
}

fn prioritize_recaptures(windows: &mut [Window], retries: &HashSet<u32>) {
    windows.sort_by_key(|window| !retries.contains(&window.window_id));
}

fn publication_delay(paced: bool, since: Duration, interval: Duration) -> Duration {
    if paced {
        Duration::ZERO
    } else {
        interval.saturating_sub(since)
    }
}
fn visible_capture_pending(backend: &dyn backend::WindowBackend) -> bool {
    backend.capture_pending()
        && backend
            .windows()
            .values()
            .any(|window| window.visible && backend.capture_needed(window.window_id))
}
mod backend;
mod desktop;
mod desktop_input;
mod discovery;
mod electron_scale;
mod input_queue;
mod launch_policy;
mod output;
mod request_error;
mod session_bus;
mod session_lifecycle;
mod video;
mod video_qos;
mod wayland;
struct Session {
    scene: Option<paneferry_protocol::SceneSize>,
    id: String,
    app_id: String,
    display: u16,
    scale_factor: u8,
    backend: Option<Box<dyn backend::WindowBackend>>,
    children: Vec<Child>,
    controller: bool,
    exited: bool,
    existing: Option<ProcessIdentity>,
    root_window: Option<u32>,
    last_existing_window_check: Option<Instant>,
    private_authority: Option<PathBuf>,
    secrets_bus: Option<session_bus::SecretsBus>,
    wayland: bool,
    desktop_monitor: Option<String>,
    window_lifetime: session_lifecycle::WindowLifetime,
    page_scale: Option<electron_scale::Lease>,
    page_scale_authority: Option<PathBuf>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct ProcessIdentity {
    pid: u32,
    started: u64,
    executable: PathBuf,
}
fn process_start_time(stat: &str) -> Result<u64> {
    // comm may contain spaces and closing parentheses. Fields after its last
    // closing parenthesis start with field 3 (state); starttime is field 22.
    Ok(stat
        .rsplit_once(')')
        .context("invalid process stat")?
        .1
        .split_whitespace()
        .nth(19)
        .context("missing process start time")?
        .parse()?)
}
fn process_identity(pid: u32) -> Result<ProcessIdentity> {
    let root = PathBuf::from(format!("/proc/{pid}"));
    if fs::metadata(&root)?.uid() != unsafe { libc::geteuid() } {
        bail!("existing process belongs to another user")
    }
    Ok(ProcessIdentity {
        pid,
        started: process_start_time(&fs::read_to_string(root.join("stat"))?)?,
        executable: fs::read_link(root.join("exe"))?,
    })
}
fn existing_unavailable_reason(app: &App) -> Option<&'static str> {
    let existing = app.existing.as_ref()?;
    let Ok(identity) = process_identity(existing.process_id) else {
        return Some("existing_process_unavailable");
    };
    if fs::canonicalize(&app.executable).ok().as_ref() != Some(&identity.executable) {
        return Some("existing_process_mismatch");
    }
    None
}
impl Session {
    fn page_scale_supported(&self, window: u32) -> bool {
        !self.wayland
            && self.root_window == Some(window)
            && self.page_scale_authority.is_some()
            && self
                .existing
                .as_ref()
                .is_some_and(|p| p.executable == std::path::Path::new(electron_scale::EXECUTABLE))
    }
    fn set_page_scale(&mut self, window: u32, scale: u8) -> Result<()> {
        paneferry_protocol::validate_scale(scale)?;
        if !self.page_scale_supported(window) {
            bail!("page_scale_unsupported")
        }
        if scale == 1 {
            if let Some(mut zoom) = self.page_scale.take() {
                zoom.stop()?;
            }
        } else if self.page_scale.is_none() {
            let backend = self
                .backend
                .as_ref()
                .context("native backend unavailable")?;
            if backend
                .windows()
                .get(&window)
                .is_none_or(|w| w.scale_factor != 1 || !w.visible)
            {
                bail!("page_scale_unsupported")
            }
            let mut zoom = electron_scale::Lease::start(
                self.existing.as_ref().unwrap(),
                window,
                self.display,
                self.page_scale_authority.as_ref().unwrap(),
            )?;
            backend.configure_native(window, zoom.size[0], zoom.size[1])?;
            zoom.commit()?;
            self.page_scale = Some(zoom);
        }
        if let Some(b) = &self.backend {
            b.refresh_capture();
        }
        Ok(())
    }

    fn scan_windows(&mut self) -> Result<Vec<Window>> {
        if let Some(zoom) = &mut self.page_scale {
            zoom.check()?;
        }
        let Some(backend) = self.backend.as_mut() else {
            return Ok(vec![]);
        };
        let now = Instant::now();
        let windows = match backend.scan() {
            Ok(windows) => windows,
            Err(error) => {
                self.window_lifetime.scan_failed(now);
                return Err(error);
            }
        };
        if self.existing.is_none() && self.desktop_monitor.is_none() {
            self.exited |= self
                .window_lifetime
                .observe(&windows, now, |id| backend.existing_window_exists(id));
        }
        Ok(windows)
    }
    fn has_exited(&mut self) -> Result<bool> {
        if let Some(existing) = &self.existing {
            self.exited |= process_identity(existing.pid).ok().as_ref() != Some(existing);
            // Input backends still validate the target/geometry for every event.
            // Full compositor inventory is only a liveness check: do not put
            // several synchronous list RPCs in front of every wheel/key event.
            // An active private readback must not make this health query wait
            // on the X server in the controller thread. Retry when it completes.
            if let Some(window) = self.root_window.filter(|_| {
                self.backend.as_ref().is_none_or(|b| b.maintenance_ready())
                    && self
                        .last_existing_window_check
                        .is_none_or(|last| last.elapsed() >= IDLE_SCAN_INTERVAL)
            }) {
                self.last_existing_window_check = Some(Instant::now());
                self.exited |= self
                    .backend
                    .as_ref()
                    .is_none_or(|backend| !backend.existing_window_exists(window));
            }
        } else {
            self.exited |= self
                .children
                .last_mut()
                .map(|c| c.try_wait())
                .transpose()?
                .flatten()
                .is_some();
        }
        if !self.exited
            && !self.controller
            && self.existing.is_none()
            && self.desktop_monitor.is_none()
            && self.window_lifetime.scan_due(Instant::now())
        {
            // Detached sessions must also disappear from discovery after their
            // last window closes. A failed scan is not evidence of exit.
            let _ = self.scan_windows();
        }
        if self.exited {
            if let Some(backend) = self.backend.take() {
                backend.release_all();
            }
            for mut child in self.children.drain(..).rev() {
                stop_child(&mut child);
            }
        }
        Ok(self.exited)
    }
}
// Do not signal a process group after its leader has been reaped: its numeric
// ID may have been reused by an unrelated process. A live owned leader pins its PID.
fn stop_child(child: &mut Child) {
    if matches!(child.try_wait(), Ok(None)) {
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.kill();
    }
    let _ = child.wait();
}
impl Drop for Session {
    fn drop(&mut self) {
        self.page_scale.take();
        if let Some(backend) = &self.backend {
            backend.release_all();
        }
        for mut c in self.children.drain(..).rev() {
            stop_child(&mut c);
        }
        if let Some(path) = &self.private_authority {
            let _ = fs::remove_file(path);
        }
    }
}
#[derive(Clone)]
struct LaunchRequest {
    scene: Option<paneferry_protocol::SceneSize>,
    session_id: String,
    app_id: String,
    scale_factor: u8,
}
#[derive(Clone)]
struct State {
    operations: Arc<Mutex<()>>,
    device_id: String,
    sharing: bool,
    sessions: HashMap<String, Arc<Mutex<Session>>>,
    requests: HashMap<String, LaunchRequest>,
    desktop_requests: HashMap<String, (String, String)>,
    apps: Vec<App>,
    desktop_apps: Vec<discovery::DesktopApp>,
    desktop: Option<discovery::Desktop>,
    running: HashMap<String, discovery::RunningWindow>,
    wayland_running: HashMap<String, wayland::Target>,
    runtime: PathBuf,
    desktop_input: desktop_input::Manager,
}
impl State {
    fn remember_launch(
        &mut self,
        request: &str,
        app: &str,
        scale: u8,
        scene: Option<paneferry_protocol::SceneSize>,
        session: &str,
    ) {
        self.requests.insert(
            request.into(),
            LaunchRequest {
                scene,
                session_id: session.into(),
                app_id: app.into(),
                scale_factor: scale,
            },
        );
    }
}
// Serialize launch/idempotency mutations without holding the shared catalog
// lock during process startup, desktop discovery or backend connection.
fn catalog_operation<T>(
    state: &Arc<Mutex<State>>,
    operation: impl FnOnce(&mut State) -> Result<T>,
) -> Result<T> {
    let gate = state.lock().unwrap().operations.clone();
    let _exclusive = gate.lock().unwrap();
    let mut work = state.lock().unwrap().clone();
    if !work.sharing {
        bail!("sharing disabled")
    }
    let result = operation(&mut work);
    let mut current = state.lock().unwrap();
    if STOP.load(Ordering::SeqCst) {
        bail!("agent stopping")
    }
    let retired = std::mem::replace(&mut current.sessions, work.sessions);
    current.requests = work.requests;
    current.desktop_requests = work.desktop_requests;
    current.running = work.running;
    current.wayland_running = work.wayland_running;
    current.desktop_apps = work.desktop_apps;
    let sharing = current.sharing;
    drop(current);
    // Last-owner Session drops may wait for child processes. Keep those waits
    // outside the catalog lock too.
    drop(retired);
    if !sharing {
        bail!("sharing disabled")
    }
    result
}

fn home() -> Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var("HOME").context("HOME required")?,
    ))
}
fn runtime() -> Result<PathBuf> {
    let p = home()?.join(".local/state/lan-remoteapp");
    fs::create_dir_all(&p)?;
    fs::set_permissions(&p, fs::Permissions::from_mode(0o700))?;
    Ok(p)
}
fn catalog() -> Result<Vec<App>> {
    let path = home()?.join(".config/lan-remoteapp/apps.json");
    if path.exists() {
        load_catalog(&path)
    } else {
        Ok(vec![])
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("paneferry-agent: {e:#}");
        std::process::exit(1)
    }
}
fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("clipboard-upload") => {
            let store = paneferry_backend_x11::clipboard_store::ClipboardStore::open()?;
            let id = store.receive(&mut std::io::BufReader::new(std::io::stdin().lock()))?;
            println!("{id}");
            Ok(())
        }
        Some("clipboard-download") => {
            let id = args.get(2).context("clipboard transfer id required")?;
            paneferry_backend_x11::clipboard_store::ClipboardStore::open()?
                .send(id, &mut std::io::stdout().lock())
        }
        Some("prepare-chatgpt") => {
            electron_scale::prepare(args.get(2).context("ChatGPT PID required")?)
        }
        Some("serve") => serve(),
        Some("bridge") => bridge(),
        Some("check") => {
            let apps = catalog()?;
            println!("custom app entries: {}", apps.len());
            match discovery::applications() {
                Ok(installed) => println!(
                    "installed applications: {} (icons: {})",
                    installed.len(),
                    installed
                        .iter()
                        .filter(|app| app.icon_png.is_some())
                        .count()
                ),
                Err(_) => println!(
                    "installed applications: unavailable (requires Python GI, GIO and GTK3)"
                ),
            }
            for cmd in [
                "Xvfb",
                "openbox",
                "xauth",
                "dbus-run-session",
                "xdg-dbus-proxy",
            ] {
                let ok = Command::new("/usr/bin/env")
                    .args(["which", cmd])
                    .status()?
                    .success();
                println!("{cmd}: {ok}");
            }
            Ok(())
        }
        _ => {
            bail!("usage: paneferry-agent serve | bridge | check | prepare-chatgpt PID")
        }
    }
}
fn bridge() -> Result<()> {
    let mut socket = UnixStream::connect(runtime()?.join("agent.sock"))
        .context("agent unavailable; start user service")?;
    let mut writer = socket.try_clone()?;
    let t = thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin(), &mut writer);
        let _ = writer.shutdown(std::net::Shutdown::Write);
    });
    std::io::copy(&mut socket, &mut std::io::stdout())?;
    let _ = socket.shutdown(std::net::Shutdown::Both);
    drop(t);
    Ok(())
}
fn serve() -> Result<()> {
    configured_video_fps_limit()?;
    let desktop = discovery::environment();
    let dir = runtime()?;
    let sock = dir.join("agent.sock");
    if sock.exists() {
        if UnixStream::connect(&sock).is_ok() {
            bail!("agent already running")
        }
        fs::remove_file(&sock)?;
    }
    let auth = dir.join("Xauthority");
    OpenOptions::new().create(true).append(true).open(&auth)?;
    fs::set_permissions(&auth, fs::Permissions::from_mode(0o600))?;
    std::env::set_var("XAUTHORITY", &auth);
    let apps = catalog()?;
    let listener = UnixListener::bind(&sock)?;
    fs::set_permissions(&sock, fs::Permissions::from_mode(0o600))?;
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
    ])?;
    thread::spawn(move || {
        if signals.forever().next().is_some() {
            STOP.store(true, Ordering::SeqCst);
        }
    });
    listener.set_nonblocking(true)?;
    let state = Arc::new(Mutex::new(State {
        operations: Arc::new(Mutex::new(())),
        device_id: paneferry_core::node::device_id(&dir)?,
        sharing: fs::read(dir.join("sharing-enabled")).map_or(true, |bytes| bytes == b"1"),
        sessions: HashMap::new(),
        requests: HashMap::new(),
        desktop_requests: HashMap::new(),
        apps,
        desktop_apps: vec![],
        desktop,
        running: HashMap::new(),
        wayland_running: HashMap::new(),
        desktop_input: desktop_input::Manager::new(dir.clone()),
        runtime: dir,
    }));
    eprintln!("paneferry-agent listening on private user socket");
    while !STOP.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((s, _)) => {
                if CLIENTS.fetch_add(1, Ordering::SeqCst) >= 16 {
                    CLIENTS.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let st = state.clone();
                thread::spawn(move || {
                    struct Count;
                    impl Drop for Count {
                        fn drop(&mut self) {
                            CLIENTS.fetch_sub(1, Ordering::SeqCst);
                        }
                    }
                    let _count = Count;
                    if let Err(_e) = client(s, st) {
                        eprintln!("client disconnected or protocol rejected");
                    }
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100))
            }
            Err(e) => return Err(e.into()),
        }
    }
    {
        let mut st = state.lock().unwrap();
        for session in st.sessions.values() {
            let mut s = session.lock().unwrap();
            for mut c in s.children.drain(..).rev() {
                stop_child(&mut c);
            }
        }
        st.sessions.clear();
    }
    fs::remove_file(sock)?;
    Ok(())
}

fn random_id() -> Result<String> {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b.iter().map(|v| format!("{v:02x}")).collect())
}
// Both application and desktop launches share the same live-session budget.
// Keep request history, so retries of retired sessions receive a terminal error.
fn retire_exited_sessions(st: &mut State) -> Result<()> {
    for session in st.sessions.values() {
        session.lock().unwrap().has_exited()?;
    }
    st.sessions
        .retain(|_, session| !session.lock().unwrap().exited);
    Ok(())
}
fn start_desktop(st: &mut State, monitor_id: &str, req: &str) -> Result<Arc<Mutex<Session>>> {
    if !identifier(monitor_id) || !identifier(req) {
        bail!("invalid desktop request")
    }
    if st.requests.contains_key(req) {
        bail!("request id conflict")
    }
    if let Some((remembered_monitor, id)) = st.desktop_requests.get(req) {
        if remembered_monitor != monitor_id {
            bail!("request id conflict")
        }
        let session = st.sessions.get(id).cloned().context("session expired")?;
        {
            let mut current = session.lock().unwrap();
            if current.has_exited()? {
                bail!("session exited; use a new request id to start again")
            }
            if current.controller {
                bail!("session already controlled")
            }
        }
        return Ok(session);
    }
    if st.desktop_requests.len() >= 4096 {
        bail!("request history limit reached; restart after saving work")
    }
    retire_exited_sessions(st)?;
    let mut previous = None;
    for session in st.sessions.values() {
        let mut current = session.lock().unwrap();
        if !current.has_exited()? && current.desktop_monitor.as_deref() == Some(monitor_id) {
            previous = Some(session.clone());
            break;
        }
    }
    if let Some(session) = previous {
        let current = session.lock().unwrap();
        if current.controller {
            bail!("session already controlled")
        }
        st.desktop_requests
            .insert(req.into(), (monitor_id.into(), current.id.clone()));
        drop(current);
        return Ok(session);
    }
    if st.sessions.len() >= 16 {
        bail!("active session limit reached")
    }
    let monitor = desktop::monitors()?
        .into_iter()
        .find(|m| m.id == monitor_id)
        .context("desktop_monitor_unavailable")?;
    let desktop = discovery::desktops(&st.desktop, &[])
        .into_iter()
        .next()
        .context("desktop_input_unavailable")?;
    let backend = desktop::Backend::connect(monitor, st.desktop_input.lease(), &desktop)?;
    let id = random_id()?;
    let session = Arc::new(Mutex::new(Session {
        scene: None,
        page_scale: None,
        page_scale_authority: None,
        window_lifetime: Default::default(),
        last_existing_window_check: None,
        id: id.clone(),
        app_id: format!("screen-{monitor_id}"),
        display: 0,
        scale_factor: 1,
        backend: Some(Box::new(backend)),
        children: vec![],
        controller: false,
        exited: false,
        existing: None,
        root_window: None,
        private_authority: None,
        secrets_bus: None,
        wayland: false,
        desktop_monitor: Some(monitor_id.into()),
    }));
    st.desktop_requests
        .insert(req.into(), (monitor_id.into(), id.clone()));
    st.sessions.insert(id, session.clone());
    Ok(session)
}
fn launch(
    st: &mut State,
    app_id: &str,
    req: &str,
    scale_factor: u8,
    scene: Option<paneferry_protocol::SceneSize>,
) -> Result<Arc<Mutex<Session>>> {
    paneferry_protocol::validate_scale(scale_factor)?;
    if let Some(scene) = scene {
        scene.validate(scale_factor)?;
    }
    if !identifier(app_id) || !identifier(req) {
        bail!("invalid launch request")
    }
    if st.desktop_requests.contains_key(req) {
        bail!("request id conflict")
    }
    if let Some(request) = st.requests.get(req) {
        if request.app_id != app_id
            || request.scale_factor != scale_factor
            || request.scene != scene
        {
            bail!("request id conflict")
        }
        let s = st
            .sessions
            .get(&request.session_id)
            .context("session expired")?
            .clone();
        {
            let mut s = s.lock().unwrap();
            if s.has_exited()? {
                bail!("session exited; use a new request id to start again")
            }
        }
        return Ok(s);
    }
    retire_exited_sessions(st)?;
    if st.requests.len() >= 4096 {
        bail!("request history limit reached; restart after saving work")
    }
    // App tiles continue existing work, including detached independent sessions.
    // Requests are scoped to this publisher; no name-based cross-device matching.
    if let Some(session) = launch_policy::unique(
        st.sessions
            .values()
            .filter(|session| {
                let session = session.lock().unwrap();
                session.desktop_monitor.is_none() && session.app_id == app_id
            })
            .cloned(),
    )? {
        let mut s = session.lock().unwrap();
        if s.controller {
            bail!("session already controlled")
        }
        if s.scene == scene && (scene.is_none() || s.scale_factor == scale_factor) {
            st.remember_launch(req, app_id, scale_factor, scene, &s.id);
            drop(s);
            return Ok(session);
        }
        if !s.wayland || s.existing.is_none() {
            bail!("session_mode_conflict")
        }
        let old_id = s.id.clone();
        s.exited = true; // Concurrent resume must not acquire the retiring lease.
        let retired_backend = s.backend.take();
        drop(s);
        st.sessions.remove(&old_id);
        // Drop the old compositor lease before acquiring the new presentation.
        // This session owns no application process; the source keeps running.
        drop(retired_backend);
    }
    if app_id.starts_with("desktop-") && !st.desktop_apps.iter().any(|a| a.id == app_id) {
        st.desktop_apps = discovery::applications()?;
    }
    let discovered = st
        .desktop_apps
        .iter()
        .find(|a| a.id == app_id)
        .map(|app| app.resolve())
        .transpose()?;
    let target_id = if discovered.is_some() {
        let sources = discovery::desktops(&st.desktop, &st.apps);
        let excluded = st
            .sessions
            .values()
            .filter_map(|session| {
                let s = session.lock().unwrap();
                (s.existing.is_none() && s.desktop_monitor.is_none()).then_some(s.display)
            })
            .collect::<Vec<_>>();
        st.running = discovery::running(&sources, &st.desktop_apps, &excluded)?.0;
        let native_inventory = wayland::inventory(&st.desktop_apps);
        let native_error = native_inventory.as_ref().err().map(ToString::to_string);
        st.wayland_running = native_inventory
            .unwrap_or_default()
            .into_iter()
            .map(|target| (target.id(), target))
            .collect();
        let candidate = launch_policy::unique(
            st.running
                .values()
                .filter(|target| target.app_id.as_deref() == Some(app_id))
                .map(|target| target.id.clone())
                .chain(
                    st.wayland_running
                        .values()
                        .filter(|t| t.catalog_id.as_deref() == Some(app_id))
                        .map(|t| t.id()),
                ),
        )?;
        if candidate.is_none()
            && discovered
                .as_ref()
                .is_some_and(wayland::native_process_running)
        {
            bail!(
                "{}",
                native_error
                    .as_deref()
                    .unwrap_or("no X11 windows belong to the approved process")
            )
        }
        candidate
    } else {
        Some(app_id.to_owned())
    };
    if let Some(target) = target_id
        .as_ref()
        .and_then(|id| st.wayland_running.get(id))
        .cloned()
    {
        for session in st.sessions.values().cloned().collect::<Vec<_>>() {
            let mut s = session.lock().unwrap();
            if s.desktop_monitor.is_some() {
                continue;
            }
            if s.wayland
                && s.existing.as_ref() == Some(&target.identity)
                && s.root_window == Some(target.info.window_id)
            {
                if s.controller {
                    bail!("session already controlled")
                }
                if s.scene == scene && (scene.is_none() || s.scale_factor == scale_factor) {
                    st.remember_launch(req, app_id, scale_factor, scene, &s.id);
                    return Ok(session.clone());
                }
                let old_id = s.id.clone();
                s.exited = true;
                let retired_backend = s.backend.take();
                drop(s);
                st.sessions.remove(&old_id);
                drop(retired_backend);
            }
        }
        if st.sessions.len() >= 16 {
            bail!("active session limit reached")
        }
        let desktop = discovery::desktops(&st.desktop, &[])
            .into_iter()
            .next()
            .context("desktop_input_unavailable")?;
        let backend = wayland::Backend::connect(
            target.clone(),
            st.desktop_input.lease(),
            &desktop,
            scale_factor,
            scene,
        )?;
        let id = random_id()?;
        let session = Arc::new(Mutex::new(Session {
            scene,
            page_scale: None,
            page_scale_authority: None,
            window_lifetime: Default::default(),
            last_existing_window_check: None,
            id: id.clone(),
            app_id: app_id.into(),
            display: 0,
            scale_factor: if scene.is_some() { scale_factor } else { 1 },
            backend: Some(Box::new(backend)),
            children: vec![],
            controller: false,
            exited: false,
            existing: Some(target.identity),
            root_window: Some(target.info.window_id),
            private_authority: None,
            secrets_bus: None,
            wayland: true,
            desktop_monitor: None,
        }));
        st.remember_launch(req, app_id, scale_factor, scene, &id);
        st.sessions.insert(id, session.clone());
        return Ok(session);
    }
    if scene.is_some() {
        bail!("application_scene_unavailable")
    }
    let target = target_id.as_ref().and_then(|id| st.running.get(id));
    let root_window = target.map(|target| target.window_id);
    let app = if let Some(target) = target {
        if process_identity(target.identity.pid).ok().as_ref() != Some(&target.identity) {
            bail!("approved existing process unavailable")
        }
        target.app()
    } else if let Some(desktop) = &discovered {
        desktop.app()
    } else {
        st.apps
            .iter()
            .find(|a| a.id == app_id)
            .cloned()
            .context("app not approved")?
    };
    if let Some(existing) = &app.existing {
        let identity = process_identity(existing.process_id)
            .context("approved existing process unavailable")?;
        if root_window.is_none() && identity.executable != fs::canonicalize(&app.executable)? {
            bail!("existing process does not match approved executable")
        }
        for session in st.sessions.values().cloned().collect::<Vec<_>>() {
            let s = session.lock().unwrap();
            if s.wayland {
                continue;
            }
            if s.display == existing.display
                && s.root_window != root_window
                && root_window.is_some_and(|window| {
                    s.backend
                        .as_ref()
                        .is_some_and(|backend| backend.includes_window(window))
                })
            {
                bail!("session already controlled")
            }
            if s.existing.as_ref() == Some(&identity)
                && s.display == existing.display
                && (s.root_window == root_window
                    || s.root_window.is_none()
                    || root_window.is_none())
            {
                if s.root_window != root_window {
                    bail!("session already controlled")
                }
                if s.controller {
                    bail!("session already controlled")
                }
                if s.scene != scene {
                    bail!("session_mode_conflict")
                }
                st.remember_launch(req, app_id, scale_factor, scene, &s.id);
                return Ok(session.clone());
            }
        }
        if st.sessions.len() >= 16 {
            bail!("active session limit reached")
        }
        let mut backend = Backend::connect_existing(
            existing.display,
            std::path::Path::new(&existing.xauthority),
            existing.process_id,
        )?;
        if backend.is_xwayland()? {
            // The user's portal controls the graphical login, not an arbitrary
            // nested compositor from a custom entry. Never send its coordinates
            // to a different desktop merely because both servers use Xwayland.
            if !discovery::desktops(&st.desktop, &[])
                .iter()
                .any(|desktop| desktop.display == existing.display)
            {
                bail!("desktop_input_unavailable")
            }
            backend.set_desktop_input(st.desktop_input.lease());
            if let Some(window) = root_window {
                if let Some(geometry) = wayland::X11Geometry::connect(existing.process_id, window)?
                {
                    backend.set_desktop_geometry(geometry);
                }
            }
        }
        backend.set_scale_factor(existing.scale_factor)?;
        if let Some(window) = root_window {
            backend.restrict_to_window(window)?;
        }
        if backend.scan()?.is_empty() {
            bail!("no X11 windows belong to the approved process")
        }
        let backend = backend::LogicalX11::new(backend)?;
        let id = random_id()?;
        let session = Arc::new(Mutex::new(Session {
            scene: None,
            page_scale: None,
            page_scale_authority: Some(PathBuf::from(&existing.xauthority)),
            window_lifetime: Default::default(),
            last_existing_window_check: None,
            id: id.clone(),
            app_id: app_id.into(),
            display: existing.display,
            scale_factor: existing.scale_factor,
            backend: Some(Box::new(backend)),
            children: vec![],
            controller: false,
            exited: false,
            existing: Some(identity),
            root_window,
            private_authority: None,
            secrets_bus: None,
            wayland: false,
            desktop_monitor: None,
        }));
        st.remember_launch(req, app_id, scale_factor, scene, &id);
        st.sessions.insert(id, session.clone());
        return Ok(session);
    }
    if st.sessions.len() >= 16 {
        bail!("active session limit reached")
    }
    let display = (100..1000)
        .find(|d| {
            !std::path::Path::new(&format!("/tmp/.X11-unix/X{d}")).exists()
                && !st
                    .sessions
                    .values()
                    .any(|s| s.lock().unwrap().display == *d)
        })
        .context("no display available")?;
    let id = random_id()?;
    let mut s = Session {
        scene: None,
        page_scale: None,
        page_scale_authority: None,
        window_lifetime: Default::default(),
        last_existing_window_check: None,
        id: id.clone(),
        app_id: app_id.into(),
        display,
        scale_factor,
        backend: None,
        children: vec![],
        controller: false,
        exited: false,
        existing: None,
        root_window: None,
        private_authority: None,
        secrets_bus: None,
        wayland: false,
        desktop_monitor: None,
    };
    let disp = format!(":{display}");
    let dpi = (u16::from(scale_factor) * 96).to_string();
    let auth = st.runtime.join("Xauthority");
    let cookie = random_id()?;
    if !Command::new("xauth")
        .arg("-f")
        .arg(&auth)
        .args(["add", &disp, "MIT-MAGIC-COOKIE-1", &cookie])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .success()
    {
        bail!("xauth failed")
    }
    s.children.push(
        Command::new("Xvfb")
            .args([
                &disp,
                "-screen",
                "0",
                // Tall/portrait clients may have a minimum height above 2160.
                // The backing desktop accommodates either frame dimension;
                // each published frame still has the independent 8 MP budget.
                "4096x4096x24",
                "-dpi",
                &dpi,
                "-nolisten",
                "tcp",
                "-auth",
            ])
            .arg(&auth)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Xvfb not installed")?,
    );
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(mut b) = Backend::connect(&disp) {
            b.set_private_display_dpi(scale_factor)?;
            b.set_scale_factor(scale_factor)?;
            b.enable_async_capture()?;
            s.backend = Some(Box::new(b));
            break;
        }
        if Instant::now() > until {
            bail!("Xvfb readiness timeout")
        };
        thread::sleep(Duration::from_millis(50));
    }
    s.children.push(
        Command::new("openbox")
            .env("DISPLAY", &disp)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("openbox not installed")?,
    );
    thread::sleep(Duration::from_millis(250));
    let mut application_args = Vec::new();
    if fs::canonicalize(&app.executable).ok().is_some_and(|path| {
        [
            std::path::Path::new("/usr/bin/xterm"),
            std::path::Path::new("/usr/bin/uxterm"),
        ]
        .contains(&path.as_path())
    }) {
        // Per-instance Xt resources, not ~/.Xresources or the desktop resource DB.
        // Bare xterm has no Ctrl+Shift+C/V bindings and Ctrl+V quotes the next key.
        application_args.extend(["-xrm", r"*VT100*translations: #override \n Ctrl Shift <Key>C: copy-selection(CLIPBOARD) \n Ctrl Shift <Key>V: insert-selection(CLIPBOARD)"]);
    }
    application_args.extend(app.argv.iter().map(String::as_str));
    let scale_argument = discovered
        .as_ref()
        .and_then(|app| app.scale_argument(scale_factor));
    if let Some(argument) = &scale_argument {
        application_args.push(argument);
    }
    let mut command = Command::new("dbus-run-session");
    command.arg("--");
    if let Some(snap) = discovered.as_ref().and_then(|app| app.snap.as_ref()) {
        use std::os::unix::fs::PermissionsExt;
        let directory = home()?.join("snap").join(snap).join("common/lan-remoteapp");
        fs::create_dir_all(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let authority = directory.join(format!("Xauthority-{}", s.id));
        s.private_authority = Some(authority.clone());
        if !Command::new("/usr/bin/xauth")
            .arg("-f")
            .arg(&auth)
            .arg("extract")
            .arg(&authority)
            .arg(&disp)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?
            .success()
        {
            bail!("xauth failed")
        }
        fs::set_permissions(&authority, fs::Permissions::from_mode(0o600))?;
        command = Command::new("/usr/bin/systemd-run");
        command
            .args(["--user", "--scope", "--collect", "--quiet"])
            .arg(&app.executable)
            .args(&application_args)
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path=/run/user/{}/bus", unsafe { libc::geteuid() }),
            )
            .env("XAUTHORITY", authority);
    } else if app.session_bus == SessionBus::DesktopSecrets {
        let bus = session_bus::SecretsBus::start(&st.runtime, &s.id)
            .map_err(|_| anyhow::anyhow!("desktop_secrets_unavailable"))?;
        command = Command::new(&app.executable);
        command
            .args(&application_args)
            .env("DBUS_SESSION_BUS_ADDRESS", bus.address());
        s.secrets_bus = Some(bus);
    } else {
        command.arg(&app.executable).args(&application_args);
    }
    if let Some(cwd) = discovered.as_ref().and_then(|app| app.cwd.as_ref()) {
        command.current_dir(cwd);
    }
    s.children.push(
        command
            .env("DISPLAY", &disp)
            .env_remove("WAYLAND_DISPLAY")
            .env("GDK_BACKEND", "x11")
            .env("QT_QPA_PLATFORM", "xcb")
            // Composition belongs to the controller, not a desktop IM daemon.
            .env("GTK_IM_MODULE", "gtk-im-context-simple")
            .env("QT_IM_MODULE", "compose")
            .env("XMODIFIERS", "@im=none")
            .env("DISABLE_WAYLAND", "1")
            .env("MOZ_ENABLE_WAYLAND", "0")
            .env("GDK_SCALE", scale_factor.to_string())
            // Xft.dpi already carries the logical DPI. GTK's pixel scaling
            // needs the inverse text adjustment; Qt 5 needs high-DPI enabled,
            // but Qt must not multiply it by an additional forced factor.
            .env("GDK_DPI_SCALE", if scale_factor == 2 { "0.5" } else { "1" })
            .env("QT_ENABLE_HIGHDPI_SCALING", "1")
            .env_remove("QT_SCALE_FACTOR")
            .env_remove("QT_FONT_DPI")
            .env_remove("QT_SCREEN_SCALE_FACTORS")
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("application start failed")?,
    );
    let session = Arc::new(Mutex::new(s));
    st.remember_launch(req, app_id, scale_factor, scene, &id);
    st.sessions.insert(id, session.clone());
    Ok(session)
}

fn wait_for_window(session: &Arc<Mutex<Session>>) -> Result<()> {
    {
        let mut s = session.lock().unwrap();
        if s.controller {
            bail!("session already controlled")
        }
        if let Some(backend) = &mut s.backend {
            backend.retry_window_adjustments();
        }
    }
    let started = Instant::now();
    loop {
        {
            let mut s = session.lock().unwrap();
            if s.controller {
                bail!("session already controlled")
            }
            if s.has_exited()? {
                bail!("application exited before creating a window")
            }
            if s.scan_windows()?.iter().any(|window| window.visible) {
                return Ok(());
            }
        }
        // Preserve the session and request key: a slow application may still
        // finish starting. Retrying must not spawn or kill another process.
        if started.elapsed() >= Duration::from_secs(10) {
            bail!("application window pending")
        }
        thread::sleep(Duration::from_millis(50));
    }
}
struct Lease(Arc<Mutex<Session>>);
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut s) = self.0.lock() {
            s.page_scale.take();
            s.controller = false;
            if let Some(b) = &mut s.backend {
                b.cancel_text();
            }
            if let Some(b) = &s.backend {
                b.release_all();
            }
        }
    }
}
fn acquire(session: Arc<Mutex<Session>>) -> Result<Lease> {
    let mut s = session.lock().unwrap();
    if s.has_exited()? {
        bail!("session exited")
    }
    if s.controller {
        bail!("session already controlled")
    };
    s.controller = true;
    if let Some(backend) = &mut s.backend {
        backend.retry_window_adjustments();
        backend.prepare_input();
        backend.resume_capture();
    }
    drop(s);
    Ok(Lease(session))
}
fn send(out: &mut output::Output, value: Value, session: &str) -> Result<()> {
    out.message(value, session)
}
fn win_value(
    w: &Window,
    kind: &str,
    existing: bool,
    desktop: bool,
    page_scale: u8,
    supported: bool,
) -> Value {
    let mut v = serde_json::to_value(w).unwrap();
    v["type"] = json!(kind);
    v["page_scale_supported"] = json!(supported);
    v["page_scale_factor"] = json!(page_scale);
    v["presentation_scale_factor"] = json!(w.scale_factor * page_scale);
    // Scale is a launch preference, not a measurement of GTK/Qt/Chromium UI.
    v["scale_mode"] = json!(if desktop {
        "source"
    } else if existing {
        "source"
    } else {
        "requested"
    });
    if w.role == "session" {
        v["surface_type"] = json!("session");
    }
    if desktop {
        v["surface_type"] = json!("desktop");
    }
    v
}
/// Presentation acknowledgements describe the client/network, not time spent
/// behind capture or window maintenance. Apply them on arrival; input and all
/// other control requests retain their original bounded FIFO and wake socket.
fn read_requests(
    input: UnixStream,
    tx: mpsc::SyncSender<Request>,
    mut notify: UnixStream,
    mut acknowledge: impl FnMut(u64),
) {
    let mut input = BufReader::new(input);
    let mut first = true;
    loop {
        match read_line_limit(&mut input, 65536) {
            Ok(Some(line)) if line.len() <= 65536 => {
                match paneferry_protocol::parse_request(&line) {
                    Ok(req) => {
                        // The first message still reaches the hello validator.
                        if !first {
                            if let Request::VideoFeedback { frame_id } = req {
                                acknowledge(frame_id);
                                continue;
                            }
                        }
                        first = false;
                        if tx.send(req).is_err() {
                            break;
                        }
                        let _ = notify.write(&[1]);
                    }
                    Err(_) => break,
                }
            }
            _ => break,
        }
    }
}
fn client(out: UnixStream, state: Arc<Mutex<State>>) -> Result<()> {
    let video_fps_limit = configured_video_fps_limit()?;
    struct Shutdown(UnixStream);
    impl Drop for Shutdown {
        fn drop(&mut self) {
            let _ = self.0.shutdown(std::net::Shutdown::Both);
        }
    }
    let _shutdown = Shutdown(out.try_clone()?);
    out.set_write_timeout(Some(Duration::from_secs(3)))?;
    let input = out.try_clone()?;
    let (tx, rx) = mpsc::sync_channel(16);
    let (mut wake, notify) = UnixStream::pair()?;
    wake.set_nonblocking(true)?;
    notify.set_nonblocking(true)?;
    let mut out = output::Output::new(out)?;
    let feedback = out.feedback_handle();
    thread::spawn(move || read_requests(input, tx, notify, |id| feedback.acknowledge(id)));
    match rx.recv_timeout(Duration::from_secs(10))? {
        Request::Hello { version: 1 } => {}
        _ => bail!("hello version 1 required"),
    }
    let mut capabilities = vec![
        "application.scene",
        "catalog",
        "display.scale",
        "window.page-scale",
        "input.pointer",
        "input.key",
        "input.authorization",
        "input.text.commit",
        "clipboard.paste",
        "clipboard.copy",
        "clipboard.cut",
        "clipboard.bundle",
        "session.resume",
        "session.list",
        "app.discovery",
        "window.list",
        "desktop.monitor",
        "node.info",
        "peer.reverse",
    ];
    if video::available() {
        capabilities.push("native.h264");
        capabilities.push("video.qos");
        capabilities.push("video.keyframe");
    }
    send(
        &mut out,
        json!({"type":"hello","capabilities":capabilities,
               "video_max_fps": if video::available() { Some(video_fps_limit) } else { None }}),
        "",
    )?;
    let mut last_capture = Instant::now() - Duration::from_secs(1);
    let mut last_scan = last_capture;
    let mut last_window_action = last_capture;
    let mut lease: Option<Lease> = None;
    let mut input_status = None;
    let mut exit_notified = false;
    let mut known: HashMap<u32, Window> = HashMap::new();
    let mut retry_windows = HashSet::new();
    let mut pending_request = None;
    while !STOP.load(Ordering::SeqCst) {
        if lease.is_some() && !state.lock().unwrap().sharing {
            send(
                &mut out,
                json!({"type":"error","code":"sharing_disabled"}),
                "",
            )?;
            break;
        }
        let mut text_pending = false;
        let mut capture_fd = None;
        let mut capture_completion_fd = None;
        let mut capture_prepare_delay = None;
        let mut capture_is_paced = false;
        let mut maintenance_ready = true;
        let mut capture_events_pending = false;
        let mut capture_pending = false;
        let mut metadata_ready = false;
        let mut event_driven_metadata = false;
        if let Some(l) = &lease {
            let mut s = l.0.lock().unwrap();
            let sid = s.id.clone();
            event_driven_metadata = s.scene.is_some();
            if !s.has_exited()? {
                if let Some(b) = &mut s.backend {
                    event_driven_metadata |= b.metadata_events();
                    for id in out.take_refreshes() {
                        if b.windows().contains_key(&id) {
                            retry_windows.insert(id);
                            b.refresh_window(id);
                        }
                    }
                    if let Err(error) = b.pump_text() {
                        eprintln!("controller text delivery failed: {error}");
                        send(
                            &mut out,
                            json!({"type":"error","code":"controller_text_failed","message":"application did not accept controller text"}),
                            &sid,
                        )?;
                    }
                    let cutting = b.cut_pending();
                    match b.pump_copy() {
                        Ok(Some(copy)) => {
                            let value = if let Some(id) = copy.bundle {
                                json!({"type":"clipboard.bundle","window_id":copy.window_id,"request_id":copy.request_id,"transfer_id":id})
                            } else {
                                json!({"type":"clipboard.data","window_id":copy.window_id,"request_id":copy.request_id,"text":copy.text})
                            };
                            send(&mut out, value, &sid)?;
                        }
                        Err(_) => send(
                            &mut out,
                            json!({"type":"error","code":if cutting {"clipboard_cut_failed"} else {"clipboard_copy_failed"},"message":"application did not provide selected text"}),
                            &sid,
                        )?,
                        Ok(None) => {}
                    }
                    text_pending = b.text_pending();
                    // Drain readiness without copying pixels. Requests already
                    // queued (or waking poll) must run before preparing pixels.
                    b.pump_capture_events(false)?;
                    capture_fd = Some(b.event_fd());
                    capture_completion_fd = b.capture_completion_fd();
                    capture_is_paced = b.capture_is_paced();
                    capture_prepare_delay = b.capture_delay(out.frame_interval());
                    maintenance_ready = b.maintenance_ready();
                    capture_events_pending = b.events_pending()?;
                    capture_pending = visible_capture_pending(b.as_ref());
                    metadata_ready = b.metadata_ready();
                }
            }
        }
        // Preserve ordering: later keys/clicks cannot overtake a text paste.
        let req = if text_pending {
            // Wake as soon as selection data arrives; retain a bounded timeout
            // for deadlines and shutdown instead of sleeping between each round trip.
            if let Some(fd) = capture_fd {
                let mut events = [
                    libc::pollfd {
                        fd,
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: capture_completion_fd.unwrap_or(-1),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                unsafe {
                    libc::poll(
                        events.as_mut_ptr(),
                        2,
                        if capture_events_pending { 0 } else { 5 },
                    );
                }
            }
            None
        } else if pending_request.is_some() {
            pending_request.take()
        } else {
            let frame_interval = out.frame_interval();
            let scan_delay = if !maintenance_ready {
                // Completion normally wakes this immediately. Keep a bounded
                // health/shutdown check if an X server stops replying.
                IDLE_SCAN_INTERVAL
            } else if metadata_ready {
                Duration::ZERO
            } else {
                scan_interval(
                    lease.is_some(),
                    !known.is_empty(),
                    last_window_action.elapsed() < Duration::from_millis(500),
                    event_driven_metadata,
                )
                .saturating_sub(last_scan.elapsed())
            };
            let frame_delay = if capture_pending {
                publication_delay(capture_is_paced, last_capture.elapsed(), frame_interval)
            } else {
                Duration::MAX
            };
            let timeout = if capture_events_pending {
                Duration::ZERO
            } else {
                scan_delay
                    .min(frame_delay)
                    .min(capture_prepare_delay.unwrap_or(Duration::MAX))
            };
            match input_queue::receive(&rx, &mut wake, capture_fd, capture_completion_fd, timeout) {
                Ok(r) => Some(r),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(_) => break,
            }
        };
        if let Some(req) = req {
            let req = input_queue::coalesce(req, &rx, &mut pending_request);
            if triggers_fast_scan(&req) {
                last_window_action = Instant::now();
            }
            let request_window = match &req {
                Request::KeyFrame { window_id }
                | Request::Configure { window_id, .. }
                | Request::PageScale { window_id, .. }
                | Request::Text { window_id, .. } => Some(*window_id),
                _ => None,
            };
            let result: Result<()> = (|| {
                match req {
                    Request::NodeInfo {} => {
                        let st = state.lock().unwrap();
                        send(
                            &mut out,
                            json!({"type":"node.info","device_id":st.device_id,"platform":"linux",
                            "name":std::env::var("HOSTNAME").unwrap_or_else(|_|"Ubuntu".into()),
                            "publishing_supported":true,"receiving_supported":true,"sharing_enabled":st.sharing}),
                            "",
                        )?;
                    }
                    Request::Sharing { enabled } => {
                        let mut st = state.lock().unwrap();
                        fs::write(
                            st.runtime.join("sharing-enabled"),
                            if enabled { b"1" } else { b"0" },
                        )?;
                        st.sharing = enabled;
                        send(
                            &mut out,
                            json!({"type":"sharing.changed","enabled":enabled}),
                            "",
                        )?;
                    }
                    Request::EnableVideo {
                        codec: _,
                        max_fps,
                        feedback,
                    } => {
                        if lease.is_some() {
                            bail!("configure video before attaching")
                        }
                        if !video::available() {
                            bail!("video encoder unavailable")
                        }
                        let max_fps = max_fps.min(video_fps_limit);
                        out.enable_video(max_fps, feedback)?;
                        send(
                            &mut out,
                            json!({"type":"video.ready", "codec":"h264", "max_fps":max_fps}),
                            "",
                        )?;
                    }
                    Request::ConfigureVideo { max_fps } => {
                        out.configure_video(max_fps.min(video_fps_limit))?
                    }
                    Request::VideoFeedback { frame_id } => out.video_feedback(frame_id),
                    Request::Sessions {} => {
                        let st = state.lock().unwrap().clone();
                        let mut sessions = Vec::new();
                        for session in st.sessions.values() {
                            let mut s = session.lock().unwrap();
                            if s.has_exited()? {
                                continue;
                            }
                            let controlled = s.controller;
                            let (window_count, unavailable_reason) = if controlled {
                                (
                                    s.backend.as_ref().map(|b| {
                                        b.windows().values().filter(|w| w.visible).count()
                                    }),
                                    None,
                                )
                            } else {
                                match s.scan_windows() {
                                    Ok(windows) => {
                                        (Some(windows.iter().filter(|w| w.visible).count()), None)
                                    }
                                    Err(error) => {
                                        (None, Some(request_error::code(&error.to_string())))
                                    }
                                }
                            };
                            if s.has_exited()? {
                                continue;
                            }
                            sessions.push(json!({"session_id":s.id,"app_id":s.app_id,"backend":"native","display":s.display,
                                "controlled":controlled,"window_count":window_count,"unavailable_reason":unavailable_reason,
                                "mode":if s.desktop_monitor.is_some() {"desktop"} else if s.scene.is_some() {"scene"} else {"app"},
                                "monitor_id":s.desktop_monitor}));
                        }
                        sessions.sort_by(|a, b| {
                            a["session_id"].as_str().cmp(&b["session_id"].as_str())
                        });
                        send(
                            &mut out,
                            json!({"type":"session.list","sessions":sessions}),
                            "",
                        )?;
                    }
                    Request::List {} => {
                        if !state.lock().unwrap().sharing {
                            bail!("sharing disabled")
                        }
                        let result = discovery::applications();
                        let warning = result
                            .as_ref()
                            .err()
                            .map(|_| "application_discovery_unavailable");
                        let st = {
                            let mut current = state.lock().unwrap();
                            current.desktop_apps = result.unwrap_or_default();
                            current.clone()
                        };
                        let mut apps: Vec<_> =
                            st.desktop_apps.iter().map(|app| app.public()).collect();
                        apps.extend(st.apps.iter().map(|a| {
                            json!({"id":a.id,"name":a.name,"description":a.description,
                            "source":"custom","unavailable_reason":existing_unavailable_reason(a)})
                        }));
                        send(
                            &mut out,
                            json!({"type":"app.list","apps":apps,"discovery_supported":true,"discovery_error":warning}),
                            "",
                        )?;
                    }
                    Request::Desktops {} => {
                        if !state.lock().unwrap().sharing {
                            bail!("sharing disabled")
                        }
                        let found = desktop::monitors();
                        let monitors: Vec<_> = found
                            .as_ref()
                            .map(|list| list.iter().map(desktop::Monitor::public).collect())
                            .unwrap_or_default();
                        let unavailable_reason = found.err().map(|_| "desktop_monitor_unavailable");
                        send(
                            &mut out,
                            json!({"type":"desktop.list","monitors":monitors,
                            "unavailable_reason":unavailable_reason}),
                            "",
                        )?;
                    }
                    Request::Windows {} => {
                        if !state.lock().unwrap().sharing {
                            bail!("sharing disabled")
                        }
                        let mut st = state.lock().unwrap().clone();
                        let sources = discovery::desktops(&st.desktop, &st.apps);
                        let excluded: Vec<_> = st
                            .sessions
                            .values()
                            .filter_map(|session| {
                                let session = session.lock().unwrap();
                                (session.existing.is_none() && session.desktop_monitor.is_none())
                                    .then_some(session.display)
                            })
                            .collect();
                        let (running, connected) =
                            discovery::running(&sources, &st.desktop_apps, &excluded)?;
                        let native = wayland::inventory(&st.desktop_apps);
                        let native_connected = native.is_ok();
                        st.wayland_running = native
                            .unwrap_or_default()
                            .into_iter()
                            .map(|t| (t.id(), t))
                            .collect();
                        let mut windows: Vec<_> = running
                            .values()
                            .map(|target| {
                                let existing_session = st.sessions.values().find_map(|session| {
                                    let session = session.lock().unwrap();
                                    (session.desktop_monitor.is_none()
                                        && !session.wayland
                                        && session.display == target.desktop.display
                                        && (session.backend.as_ref().is_some_and(|backend| {
                                            backend.includes_window(target.window_id)
                                        }) || session.existing.as_ref()
                                            == Some(&target.identity)
                                            && (session.root_window.is_none()
                                                || session.root_window == Some(target.window_id))))
                                    .then_some((session.id.clone(), session.controller))
                                });
                                target.public(
                                    existing_session
                                        .as_ref()
                                        .is_some_and(|(_, controlled)| *controlled),
                                    existing_session.map(|(id, _)| id),
                                )
                            })
                            .collect();
                        windows.extend(st.wayland_running.values().map(|target| {
                            let session = st.sessions.values().find_map(|session| {
                                let s = session.lock().unwrap();
                                (s.wayland
                                    && s.existing.as_ref() == Some(&target.identity)
                                    && s.backend
                                        .as_ref()
                                        .is_some_and(|b| b.includes_window(target.info.window_id)))
                                .then_some((s.id.clone(), s.controller))
                            });
                            target.public(
                                session.as_ref().is_some_and(|(_, controlled)| *controlled),
                                session.map(|(id, _)| id),
                            )
                        }));
                        windows.sort_by(|a, b| {
                            a["name"]
                                .as_str()
                                .cmp(&b["name"].as_str())
                                .then(a["title"].as_str().cmp(&b["title"].as_str()))
                        });
                        windows.truncate(128);
                        {
                            let mut current = state.lock().unwrap();
                            current.running = running;
                            current.wayland_running = st.wayland_running;
                        }
                        send(
                            &mut out,
                            json!({"type":"window.list","windows":windows,
                            "unavailable_reason":if connected || native_connected {None} else {Some("desktop_unavailable")}}),
                            "",
                        )?;
                    }
                    Request::Launch {
                        app_id,
                        request_id,
                        backend: _,
                        attach,
                        scale_factor,
                        scene,
                    } => {
                        if !state.lock().unwrap().sharing {
                            bail!("sharing disabled")
                        }
                        if lease.is_some() {
                            bail!("detach before launching another session")
                        };
                        let session = catalog_operation(&state, |st| {
                            launch(st, &app_id, &request_id, scale_factor, scene)
                        })?;
                        wait_for_window(&session)?;
                        // A launcher only prepares a session. It must not race the
                        // WindowHost for the exclusive controller lease.
                        let new = if attach {
                            Some(acquire(session.clone())?)
                        } else {
                            None
                        };
                        {
                            let s = session.lock().unwrap();
                            send(
                                &mut out,
                                json!({"type":"session.started","backend":"native","display":s.display,"scale_factor":s.scale_factor}),
                                &s.id,
                            )?;
                        }
                        lease = new;
                    }
                    Request::StartDesktop {
                        monitor_id,
                        request_id,
                    } => {
                        if !state.lock().unwrap().sharing {
                            bail!("sharing disabled")
                        }
                        if lease.is_some() {
                            bail!("detach before starting another session")
                        }
                        let session = catalog_operation(&state, |st| {
                            start_desktop(st, &monitor_id, &request_id)
                        })?;
                        let s = session.lock().unwrap();
                        send(
                            &mut out,
                            json!({"type":"session.started","backend":"native",
                            "display":s.display,"scale_factor":1,"mode":"desktop"}),
                            &s.id,
                        )?;
                    }
                    Request::Resume { session_id } => {
                        if !state.lock().unwrap().sharing {
                            bail!("sharing disabled")
                        }
                        if lease.is_some() {
                            bail!("already attached")
                        };
                        let session = state
                            .lock()
                            .unwrap()
                            .sessions
                            .get(&session_id)
                            .cloned()
                            .context("unknown session")?;
                        let new = acquire(session)?;
                        {
                            let mut s = new.0.lock().unwrap();
                            let windows = s.scan_windows()?;
                            known = windows.iter().cloned().map(|w| (w.window_id, w)).collect();
                            let values: Vec<_> = windows
                                .iter()
                                .map(|w| {
                                    win_value(
                                        w,
                                        "window.created",
                                        s.existing.is_some(),
                                        s.desktop_monitor.is_some(),
                                        if s.page_scale
                                            .as_ref()
                                            .is_some_and(|z| z.window == w.window_id)
                                        {
                                            2
                                        } else {
                                            1
                                        },
                                        s.page_scale_supported(w.window_id),
                                    )
                                })
                                .collect();
                            send(
                                &mut out,
                                json!({"type":"session.snapshot","windows":values,
                                    "mode":if s.desktop_monitor.is_some() {"desktop"} else if s.scene.is_some() {"scene"} else {"app"}}),
                                &s.id,
                            )?;
                        }
                        lease = Some(new);
                    }
                    Request::Detach {} => {
                        bail!("detach")
                    }
                    Request::Hello { .. } => bail!("duplicate hello"),
                    Request::PageScale { window_id, scale } => {
                        let l = lease.as_ref().context("no session attached")?;
                        let mut s = l.0.lock().unwrap();
                        s.set_page_scale(window_id, scale)?;
                        known.clear();
                    }
                    request => {
                        let l = lease.as_ref().context("no session attached")?;
                        let mut s = l.0.lock().unwrap();
                        if s.has_exited()? {
                            bail!("session exited")
                        }
                        let b = s.backend.as_mut().context("native backend unavailable")?;
                        match request {
                            Request::Configure {
                                window_id,
                                width,
                                height,
                            } => b.configure(window_id, width, height)?,
                            Request::KeyFrame { window_id } => {
                                if !b.windows().get(&window_id).is_some_and(|w| w.visible) {
                                    bail!("window unavailable");
                                }
                                out.keyframe(window_id)?;
                                b.refresh_capture();
                            }
                            Request::Focus { window_id } => b.focus(window_id)?,
                            Request::Close { window_id } => b.close(window_id)?,
                            Request::Pointer {
                                window_id,
                                x,
                                y,
                                button,
                                pressed,
                            } => b.pointer(window_id, x, y, button, pressed)?,
                            Request::Key {
                                window_id,
                                keysym,
                                pressed,
                            } => b.key(window_id, keysym, pressed)?,
                            Request::Text { window_id, text } => b.commit_text(window_id, text)?,
                            Request::Paste { window_id, text } => b.paste_text(window_id, text)?,
                            Request::Copy {
                                window_id,
                                request_id,
                            } => b.copy_text(window_id, request_id)?,
                            Request::Cut {
                                window_id,
                                request_id,
                            } => b.cut_text(window_id, request_id)?,
                            Request::CopyBundle {
                                window_id,
                                request_id,
                                cut,
                            } => b.copy_bundle(window_id, request_id, cut)?,
                            Request::PasteBundle {
                                window_id,
                                transfer_id,
                            } => b.paste_bundle(window_id, &transfer_id)?,
                            _ => unreachable!(),
                        }
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                if e.to_string() == "detach" {
                    break;
                };
                let sid = lease
                    .as_ref()
                    .map(|l| l.0.lock().unwrap().id.clone())
                    .unwrap_or_default();
                let message = e.to_string();
                let code = request_error::code(&message);
                send(
                    &mut out,
                    json!({"type":"error","code":code,"message":message,"window_id":request_window}),
                    &sid,
                )?;
            }
        }
        // Preparing capture pixels is asynchronous and nonblocking. Feed raw
        // samples and collect converted samples as they arrive, after input.
        // Gating both steps on the video deadline makes each pipeline stage
        // wait another frame interval and retains producer buffers meanwhile.
        // Only encoding/publication below is limited by the negotiated FPS.
        if let Some(l) = &lease {
            let mut s = l.0.lock().unwrap();
            if let Some(b) = s.backend.as_mut() {
                b.pump_capture_events(false)?;
                metadata_ready = b.metadata_ready();
                let maintenance_due = metadata_ready
                    || last_scan.elapsed()
                        >= scan_interval(
                            lease.is_some(),
                            !known.is_empty(),
                            last_window_action.elapsed() < Duration::from_millis(500),
                            event_driven_metadata,
                        );
                if !maintenance_due || b.capture_completion_fd().is_none() {
                    b.prepare_capture(out.frame_interval())?;
                }
                maintenance_ready = b.maintenance_ready();
                capture_is_paced = b.capture_is_paced();
                capture_pending = visible_capture_pending(b.as_ref());
            }
        }
        // The current input has already run. Do not make continuous input
        // postpone visual feedback beyond the negotiated frame deadline.
        let frame_interval = out.frame_interval();
        let frame_due = capture_pending
            && publication_delay(capture_is_paced, last_capture.elapsed(), frame_interval)
                .is_zero();
        if input_queue::defer_maintenance(&rx, &mut pending_request, last_scan.elapsed(), frame_due)
        {
            continue;
        }
        let scan_due = maintenance_ready
            && (metadata_ready
                || last_scan.elapsed()
                    >= scan_interval(
                        lease.is_some(),
                        !known.is_empty(),
                        last_window_action.elapsed() < Duration::from_millis(500),
                        event_driven_metadata,
                    ));
        let capture_due = capture_pending
            && publication_delay(capture_is_paced, last_capture.elapsed(), frame_interval)
                .is_zero();
        if !scan_due && !capture_due {
            continue;
        }
        if scan_due {
            last_scan = Instant::now();
        }
        if let Some(l) = &lease {
            let mut s = l.0.lock().unwrap();
            let sid = s.id.clone();
            let mut windows = if !s.has_exited()? && scan_due {
                match s.scan_windows() {
                    Ok(windows) => windows,
                    Err(error) => {
                        let code = request_error::code(&error.to_string());
                        send(&mut out, json!({"type":"error","code":code}), &sid)?;
                        // Capture/bridge failures only detach this controller.
                        break;
                    }
                }
            } else {
                s.backend
                    .as_ref()
                    .map(|b| b.windows().values().cloned().collect())
                    .unwrap_or_default()
            };
            if s.has_exited()? {
                if !exit_notified {
                    for mut c in s.children.drain(..).rev() {
                        stop_child(&mut c);
                    }
                    send(&mut out, json!({"type":"session.exited"}), &sid)?;
                    exit_notified = true;
                }
                continue;
            }
            let existing = s.existing.is_some();
            let desktop_mode = s.desktop_monitor.is_some();
            let scaled_window = s.page_scale.as_ref().map(|z| z.window);
            let scale_supported = s.root_window.filter(|id| s.page_scale_supported(*id));
            if let Some(b) = s.backend.as_mut() {
                let status = b.input_status();
                if status != input_status {
                    input_status = status;
                    if let Some(status) = status {
                        send(
                            &mut out,
                            json!({"type":"input.authorization","state":status}),
                            &sid,
                        )?;
                    }
                }
                // Metadata/input-status queries can outlive the frame deadline
                // or a conversion completion. Select again after this work,
                // immediately before publication, without waiting for conversion.
                // Pair any new pixels with their updated geometry before sending
                // metadata, including destroy events drained by X11 here.
                let due_after_maintenance =
                    b.capture_is_paced() || last_capture.elapsed() >= out.frame_interval();
                if due_after_maintenance {
                    b.prepare_capture(out.frame_interval())?;
                    windows = b.windows().values().cloned().collect();
                }
                retry_windows.retain(|id| {
                    windows
                        .iter()
                        .any(|window| window.window_id == *id && window.visible)
                });
                // A continuously repainting large window must not always get the
                // first admission attempt ahead of a previously skipped snapshot.
                prioritize_recaptures(&mut windows, &retry_windows);
                for id in known.keys().filter(|id| !b.windows().contains_key(id)) {
                    send(
                        &mut out,
                        json!({"type":"window.destroyed","window_id":id}),
                        &sid,
                    )?;
                }
                let mut attempted_capture = false;
                for w in &windows {
                    let old = known.get(&w.window_id);
                    if old != Some(w) {
                        send(
                            &mut out,
                            win_value(
                                w,
                                if old.is_some() {
                                    "window.metadata"
                                } else {
                                    "window.created"
                                },
                                existing,
                                desktop_mode,
                                if scaled_window == Some(w.window_id) {
                                    2
                                } else {
                                    1
                                },
                                scale_supported == Some(w.window_id),
                            ),
                            &sid,
                        )?;
                    }
                    if !due_after_maintenance || !w.visible || !b.capture_needed(w.window_id) {
                        continue;
                    }
                    if pending_request.is_none() {
                        pending_request = rx.try_recv().ok();
                    }
                    // Admit one due capture before returning to queued input.
                    // Other dirty windows keep their turn for the next frame.
                    if attempted_capture && pending_request.is_some() {
                        retry_windows.insert(w.window_id);
                        continue;
                    }
                    attempted_capture = true;
                    // Metadata-only scans must not postpone the first repaint
                    // after idle. Failed attempts still retain the adaptive cap.
                    last_capture = Instant::now();
                    if let Ok(image) = b.capture(w) {
                        retry_windows.remove(&w.window_id);
                        out.frame(w, image, &sid)?;
                    }
                }
                known = windows.into_iter().map(|w| (w.window_id, w)).collect();
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paced_readbacks_publish_when_complete_without_charging_another_frame_period() {
        let interval = Duration::from_micros(33_333);
        let mut last_published = Duration::ZERO;
        for started in [0, 33_333, 66_666, 99_999] {
            let ready = Duration::from_micros(started + 25_000);
            let published = ready + publication_delay(true, ready - last_published, interval);
            assert_eq!(published, ready, "readback already admitted at 30 FPS");
            last_published = published;
        }
        assert_eq!(
            publication_delay(false, Duration::from_micros(25_000), interval),
            Duration::from_micros(8_333),
            "an unpaced producer still needs the output FPS gate"
        );
    }
    #[test]
    fn request_reader_applies_feedback_without_waiting_for_control_dispatch() {
        let (input, mut controller) = UnixStream::pair().unwrap();
        let (notify, _wake) = UnixStream::pair().unwrap();
        let (tx, rx) = mpsc::sync_channel(16);
        let (feedback, applied) = mpsc::channel();
        let task = thread::spawn(move || {
            read_requests(input, tx, notify, |id| feedback.send(id).unwrap())
        });
        controller.write_all(concat!(
            "{\"version\":1,\"type\":\"hello\"}\n",
            "{\"version\":1,\"type\":\"input.key\",\"window_id\":42,\"keysym\":97,\"pressed\":true}\n",
            "{\"version\":1,\"type\":\"video.feedback\",\"frame_id\":7}\n",
            "{\"version\":1,\"type\":\"input.key\",\"window_id\":42,\"keysym\":97,\"pressed\":false}\n",
        ).as_bytes()).unwrap();
        // The controller/capture owner has not consumed even hello yet.
        assert_eq!(applied.recv_timeout(Duration::from_secs(1)).unwrap(), 7);
        assert!(matches!(rx.recv().unwrap(), Request::Hello { version: 1 }));
        assert!(matches!(
            rx.recv().unwrap(),
            Request::Key { pressed: true, .. }
        ));
        assert!(matches!(
            rx.recv().unwrap(),
            Request::Key { pressed: false, .. }
        ));
        drop(controller);
        task.join().unwrap();
    }
    #[test]
    fn feedback_cannot_bypass_the_initial_hello_validation() {
        let (input, mut controller) = UnixStream::pair().unwrap();
        let (notify, _wake) = UnixStream::pair().unwrap();
        let (tx, rx) = mpsc::sync_channel(16);
        let task = thread::spawn(move || {
            read_requests(input, tx, notify, |_| {
                panic!("first request bypassed hello")
            })
        });
        controller
            .write_all(b"{\"version\":1,\"type\":\"video.feedback\",\"frame_id\":7}\n")
            .unwrap();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Request::VideoFeedback { frame_id: 7 }
        ));
        drop(controller);
        task.join().unwrap();
    }
    #[test]
    fn publisher_video_limit_preserves_default_and_rejects_invalid_configuration() {
        assert_eq!(
            video_fps_limit(None).unwrap(),
            paneferry_protocol::MAX_VIDEO_FPS
        );
        for cap in ["1", "15", "30", "60", "90"] {
            assert_eq!(
                video_fps_limit(Some(cap)).unwrap(),
                cap.parse::<u8>().unwrap()
            );
        }
        for invalid in ["", "0", "91", "256", "-1", "30.0", "auto"] {
            assert!(video_fps_limit(Some(invalid)).is_err(), "{invalid}");
        }
    }

    #[test]
    fn skipped_static_windows_precede_continuously_repainting_windows() {
        let make = |id| Window {
            window_id: id,
            parent_id: None,
            title: String::new(),
            presentation: None,
            role: "toplevel".into(),
            x: 0,
            y: 0,
            width: 3000,
            height: 2000,
            scale_factor: 1,
            geometry_revision: 1,
            visible: true,
        };
        let mut windows = vec![make(1), make(2), make(3)];
        prioritize_recaptures(&mut windows, &HashSet::from([2, 3]));
        assert_eq!(
            windows.iter().map(|w| w.window_id).collect::<Vec<_>>(),
            vec![2, 3, 1]
        );
        // If only the first retry fits, the next scan gives the remaining one
        // first choice rather than putting the continuously dirty window first.
        prioritize_recaptures(&mut windows, &HashSet::from([3, 1]));
        assert_eq!(
            windows.iter().map(|w| w.window_id).collect::<Vec<_>>(),
            vec![3, 1, 2]
        );
    }

    #[test]
    fn metadata_scans_keep_fast_discovery_without_polling_composed_popups() {
        assert_eq!(
            scan_interval(false, false, false, false),
            Duration::from_secs(1)
        );
        assert_eq!(
            scan_interval(true, false, false, false),
            ACTIVE_SCAN_INTERVAL
        );
        assert_eq!(scan_interval(true, true, true, false), ACTIVE_SCAN_INTERVAL);
        assert_eq!(scan_interval(true, true, false, false), IDLE_SCAN_INTERVAL);
        // Popups inside a composed canvas arrive with PipeWire frames, not
        // newly discovered streams. Still discover the first canvas promptly.
        assert_eq!(scan_interval(true, false, true, true), ACTIVE_SCAN_INTERVAL);
        assert_eq!(scan_interval(true, true, true, true), IDLE_SCAN_INTERVAL);
        assert_eq!(scan_interval(true, true, false, true), IDLE_SCAN_INTERVAL);
        assert!(triggers_fast_scan(&Request::Pointer {
            window_id: 1,
            x: 10,
            y: 10,
            button: 1,
            pressed: true,
        }));
        assert!(!triggers_fast_scan(&Request::Pointer {
            window_id: 1,
            x: 10,
            y: 10,
            button: 0,
            pressed: false,
        }));
    }
    #[test]
    fn app_reuses_detached_session_and_remembers_original_request_parameters() {
        let child = Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let session = Arc::new(Mutex::new(Session {
            scene: None,
            page_scale: None,
            page_scale_authority: None,
            window_lifetime: Default::default(),
            last_existing_window_check: None,
            id: "existing-session".into(),
            app_id: "test-app".into(),
            display: 101,
            scale_factor: 1,
            backend: None,
            children: vec![child],
            controller: false,
            exited: false,
            existing: None,
            root_window: None,
            private_authority: None,
            secrets_bus: None,
            wayland: false,
            desktop_monitor: None,
        }));
        let mut st = State {
            operations: Arc::new(Mutex::new(())),
            device_id: "test-device".into(),
            sharing: true,
            sessions: HashMap::from([("existing-session".into(), session.clone())]),
            requests: HashMap::new(),
            desktop_requests: HashMap::new(),
            apps: vec![],
            desktop_apps: vec![],
            desktop: None,
            running: HashMap::new(),
            wayland_running: HashMap::new(),
            runtime: PathBuf::from("/unused"),
            desktop_input: desktop_input::Manager::new(PathBuf::from("/unused")),
        };
        assert!(Arc::ptr_eq(
            &launch(&mut st, "test-app", "request", 2, None).unwrap(),
            &session
        ));
        assert!(Arc::ptr_eq(
            &launch(&mut st, "test-app", "request", 2, None).unwrap(),
            &session
        ));
        assert_eq!(
            session.lock().unwrap().scale_factor,
            1,
            "reuse must not change application scale"
        );
        assert!(launch(&mut st, "other-app", "request", 2, None).is_err());
        assert!(launch(&mut st, "test-app", "request", 1, None).is_err());
        let canvas = Some(paneferry_protocol::SceneSize {
            width: 1600,
            height: 900,
        });
        assert!(launch(&mut st, "test-app", "request", 2, canvas)
            .err()
            .unwrap()
            .to_string()
            .contains("request id conflict"));
        assert!(launch(&mut st, "test-app", "scene-request", 2, canvas)
            .err()
            .unwrap()
            .to_string()
            .contains("session_mode_conflict"));
        session.lock().unwrap().controller = true;
        assert!(launch(&mut st, "test-app", "another-request", 2, None).is_err());
        assert_eq!(st.sessions.len(), 1);
        // A cached desktop request must not announce a retired session as started.
        session.lock().unwrap().controller = false;
        session.lock().unwrap().exited = true;
        st.desktop_requests.insert(
            "desktop-request".into(),
            ("monitor".into(), "existing-session".into()),
        );
        assert_eq!(
            start_desktop(&mut st, "monitor", "desktop-request")
                .err()
                .unwrap()
                .to_string(),
            "session exited; use a new request id to start again"
        );
        retire_exited_sessions(&mut st).unwrap();
        assert!(st.sessions.is_empty());
        assert!(st.requests.contains_key("request"));
        assert!(st.desktop_requests.contains_key("desktop-request"));
        assert_eq!(
            start_desktop(&mut st, "monitor", "desktop-request")
                .err()
                .unwrap()
                .to_string(),
            "session expired"
        );
        let shared = Arc::new(Mutex::new(st));
        let worker = shared.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let task = thread::spawn(move || {
            catalog_operation(&worker, |_| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        // A stalled launch/discovery cannot block an attached loop or stop-sharing.
        shared
            .try_lock()
            .expect("global lock held during slow operation")
            .sharing = false;
        assert!(session.try_lock().is_ok());
        release_tx.send(()).unwrap();
        assert!(task.join().unwrap().is_err());
        assert!(!shared.lock().unwrap().sharing);
    }
    #[test]
    fn id_entropy() {
        assert_eq!(random_id().unwrap().len(), 32);
        assert_ne!(random_id().unwrap(), random_id().unwrap());
    }
    #[test]
    fn process_identity_parses_commands_with_spaces_and_parentheses() {
        let tail = std::iter::once("S")
            .chain(std::iter::repeat_n("0", 18))
            .chain(std::iter::once("12345"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            process_start_time(&format!("123 (App with ) spaces) {tail} 0 0")).unwrap(),
            12345
        );
        assert!(process_start_time("123 (App) S 0").is_err());
    }
}
