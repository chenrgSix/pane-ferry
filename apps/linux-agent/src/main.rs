#[cfg(not(unix))]
compile_error!("Linux agent requires Unix");
use anyhow::{bail, Context, Result};
use remoteapp_backend_x11::Backend;
use remoteapp_core::{load_catalog, App};
use remoteapp_protocol::{identifier, read_line_limit, Request, Window};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
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
mod discovery;
mod input_queue;
mod output;
mod request_error;
mod video;
struct Session {
    id: String,
    app_id: String,
    display: u16,
    scale_factor: u8,
    backend: Option<Backend>,
    children: Vec<Child>,
    controller: bool,
    exited: bool,
    existing: Option<ProcessIdentity>,
    root_window: Option<u32>,
    private_authority: Option<PathBuf>,
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
    fn has_exited(&mut self) -> Result<bool> {
        if let Some(existing) = &self.existing {
            self.exited |= process_identity(existing.pid).ok().as_ref() != Some(existing);
            if let Some(window) = self.root_window {
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
struct State {
    sessions: HashMap<String, Arc<Mutex<Session>>>,
    requests: HashMap<String, String>,
    apps: Vec<App>,
    desktop_apps: Vec<discovery::DesktopApp>,
    desktop: Option<discovery::Desktop>,
    running: HashMap<String, discovery::RunningWindow>,
    runtime: PathBuf,
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
        eprintln!("remoteapp-agent: {e:#}");
        std::process::exit(1)
    }
}
fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
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
            for cmd in ["Xvfb", "openbox", "xauth", "dbus-run-session"] {
                let ok = Command::new("/usr/bin/env")
                    .args(["which", cmd])
                    .status()?
                    .success();
                println!("{cmd}: {ok}");
            }
            Ok(())
        }
        _ => {
            bail!("usage: remoteapp-agent serve | bridge | check")
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
        sessions: HashMap::new(),
        requests: HashMap::new(),
        apps,
        desktop_apps: vec![],
        desktop,
        running: HashMap::new(),
        runtime: dir,
    }));
    eprintln!("remoteapp-agent listening on private user socket");
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
fn launch(
    st: &mut State,
    app_id: &str,
    req: &str,
    scale_factor: u8,
) -> Result<Arc<Mutex<Session>>> {
    remoteapp_protocol::validate_scale(scale_factor)?;
    if !identifier(app_id) || !identifier(req) {
        bail!("invalid launch request")
    }
    if let Some(sid) = st.requests.get(req) {
        let s = st.sessions.get(sid).context("session expired")?.clone();
        {
            let mut s = s.lock().unwrap();
            if s.app_id != app_id || s.existing.is_none() && s.scale_factor != scale_factor {
                bail!("request id conflict")
            }
            if s.has_exited()? {
                bail!("session exited; use a new request id to start again")
            }
        }
        return Ok(s);
    }
    for session in st.sessions.values() {
        let mut s = session.lock().unwrap();
        s.has_exited()?;
    }
    st.sessions.retain(|_, s| !s.lock().unwrap().exited);
    if st.requests.len() >= 4096 {
        bail!("request history limit reached; restart after saving work")
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
    let root_window = st.running.get(app_id).map(|target| target.window_id);
    let app = if let Some(target) = st.running.get(app_id) {
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
        for session in st.sessions.values() {
            let s = session.lock().unwrap();
            if s.existing.as_ref() == Some(&identity)
                && s.display == existing.display
                && (s.root_window == root_window
                    || s.root_window.is_none()
                    || root_window.is_none())
            {
                if s.root_window != root_window {
                    bail!("session already controlled")
                }
                if s.app_id != app_id {
                    bail!("existing process already registered under another app")
                }
                st.requests.insert(req.into(), s.id.clone());
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
        backend.set_scale_factor(existing.scale_factor)?;
        if let Some(window) = root_window {
            backend.restrict_to_window(window)?;
        }
        if backend.scan()?.is_empty() {
            bail!("no X11 windows belong to the approved process")
        }
        let id = random_id()?;
        let session = Arc::new(Mutex::new(Session {
            id: id.clone(),
            app_id: app_id.into(),
            display: existing.display,
            scale_factor: existing.scale_factor,
            backend: Some(backend),
            children: vec![],
            controller: false,
            exited: false,
            existing: Some(identity),
            root_window,
            private_authority: None,
        }));
        st.requests.insert(req.into(), id.clone());
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
    };
    let disp = format!(":{display}");
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
                "4096x2160x24",
                "-dpi",
                "96",
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
            b.set_scale_factor(scale_factor)?;
            s.backend = Some(b);
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
            .env("DISABLE_WAYLAND", "1")
            .env("MOZ_ENABLE_WAYLAND", "0")
            .env("GDK_SCALE", scale_factor.to_string())
            .env("GDK_DPI_SCALE", "1")
            .env("QT_SCALE_FACTOR", scale_factor.to_string())
            .env("QT_FONT_DPI", "96")
            .env_remove("QT_SCREEN_SCALE_FACTORS")
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("application start failed")?,
    );
    let session = Arc::new(Mutex::new(s));
    st.requests.insert(req.into(), id.clone());
    st.sessions.insert(id, session.clone());
    Ok(session)
}
struct Lease(Arc<Mutex<Session>>);
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut s) = self.0.lock() {
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
    if let Some(backend) = &s.backend {
        backend.refresh_capture();
    }
    drop(s);
    Ok(Lease(session))
}
fn send(out: &mut output::Output, value: Value, session: &str) -> Result<()> {
    out.message(value, session)
}
fn win_value(w: &Window, kind: &str) -> Value {
    let mut v = serde_json::to_value(w).unwrap();
    v["type"] = json!(kind);
    v
}
fn client(out: UnixStream, state: Arc<Mutex<State>>) -> Result<()> {
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
    let (mut wake, mut notify) = UnixStream::pair()?;
    wake.set_nonblocking(true)?;
    notify.set_nonblocking(true)?;
    thread::spawn(move || {
        let mut input = BufReader::new(input);
        loop {
            match read_line_limit(&mut input, 65536) {
                Ok(Some(line)) if line.len() <= 65536 => {
                    match remoteapp_protocol::parse_request(&line) {
                        Ok(req) => {
                            if tx.send(req).is_err() {
                                break;
                            }
                            // A full wake socket is already readable. Never
                            // block the request reader on notification bytes.
                            let _ = notify.write(&[1]);
                        }
                        Err(_) => break,
                    }
                }
                _ => break,
            }
        }
    });
    let mut out = output::Output::new(out)?;
    match rx.recv_timeout(Duration::from_secs(10))? {
        Request::Hello { version: 1 } => {}
        _ => bail!("hello version 1 required"),
    }
    let mut capabilities = vec![
        "catalog",
        "native.png",
        "display.scale",
        "input.pointer",
        "input.key",
        "input.text.commit",
        "clipboard.paste",
        "clipboard.copy",
        "session.resume",
        "session.list",
        "app.discovery",
        "window.list",
    ];
    if video::available() {
        capabilities.push("native.h264");
    }
    send(
        &mut out,
        json!({"type":"hello","capabilities":capabilities}),
        "",
    )?;
    let mut last_capture = Instant::now() - Duration::from_secs(1);
    let mut last_scan = last_capture;
    let mut lease: Option<Lease> = None;
    let mut exit_notified = false;
    let mut known: HashMap<u32, Window> = HashMap::new();
    let mut pending_request = None;
    while !STOP.load(Ordering::SeqCst) {
        let mut text_pending = false;
        let mut x11_fd = None;
        let mut capture_pending = false;
        if let Some(l) = &lease {
            let mut s = l.0.lock().unwrap();
            let sid = s.id.clone();
            if !s.has_exited()? {
                if let Some(b) = &mut s.backend {
                    if let Err(error) = b.pump_text() {
                        eprintln!("controller text delivery failed: {error}");
                        send(
                            &mut out,
                            json!({"type":"error","code":"controller_text_failed","message":"application did not accept controller text"}),
                            &sid,
                        )?;
                    }
                    match b.pump_copy() {
                        Ok(Some(copy)) => send(
                            &mut out,
                            json!({"type":"clipboard.data","window_id":copy.window_id,"request_id":copy.request_id,"text":copy.text}),
                            &sid,
                        )?,
                        Err(_) => send(
                            &mut out,
                            json!({"type":"error","code":"clipboard_copy_failed","message":"application did not provide selected text"}),
                            &sid,
                        )?,
                        Ok(None) => {}
                    }
                    text_pending = b.text_pending();
                    b.pump_capture_events()?;
                    x11_fd = Some(b.event_fd());
                    capture_pending = b.capture_pending();
                }
            }
        }
        // Preserve ordering: later keys/clicks cannot overtake a text paste.
        let req = if text_pending {
            // Wake as soon as selection data arrives; retain a bounded timeout
            // for deadlines and shutdown instead of sleeping between each round trip.
            if let Some(fd) = x11_fd {
                let mut event = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                unsafe {
                    libc::poll(&mut event, 1, 5);
                }
            }
            None
        } else if pending_request.is_some() {
            pending_request.take()
        } else {
            let scan_delay = Duration::from_millis(33).saturating_sub(last_scan.elapsed());
            let frame_delay = if capture_pending {
                Duration::from_millis(33).saturating_sub(last_capture.elapsed())
            } else {
                Duration::MAX
            };
            match input_queue::receive(&rx, &mut wake, x11_fd, scan_delay.min(frame_delay)) {
                Ok(r) => Some(r),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(_) => break,
            }
        };
        if let Some(req) = req {
            let req = input_queue::coalesce(req, &rx, &mut pending_request);
            let result: Result<()> = (|| {
                match req {
                    Request::EnableVideo { codec: _ } => {
                        if lease.is_some() {
                            bail!("configure video before attaching")
                        }
                        if !video::available() {
                            bail!("video encoder unavailable")
                        }
                        out.enable_video()?;
                        send(&mut out, json!({"type":"video.ready", "codec":"h264"}), "")?;
                    }
                    Request::Sessions {} => {
                        let st = state.lock().unwrap();
                        let mut sessions = Vec::new();
                        for session in st.sessions.values() {
                            let mut s = session.lock().unwrap();
                            if s.has_exited()? {
                                continue;
                            }
                            sessions.push(json!({"session_id":s.id,"app_id":s.app_id,"backend":"native","display":s.display,"controlled":s.controller}));
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
                        let result = discovery::applications();
                        let warning = result
                            .as_ref()
                            .err()
                            .map(|_| "application_discovery_unavailable");
                        let mut st = state.lock().unwrap();
                        st.desktop_apps = result.unwrap_or_default();
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
                    Request::Windows {} => {
                        let mut st = state.lock().unwrap();
                        let sources = discovery::desktops(&st.desktop, &st.apps);
                        let excluded: Vec<_> = st
                            .sessions
                            .values()
                            .filter_map(|session| {
                                let session = session.lock().unwrap();
                                session.existing.is_none().then_some(session.display)
                            })
                            .collect();
                        let (running, connected) =
                            discovery::running(&sources, &st.desktop_apps, &excluded)?;
                        let mut windows: Vec<_> = running
                            .values()
                            .map(|target| {
                                let existing_session = st.sessions.values().find_map(|session| {
                                    let session = session.lock().unwrap();
                                    (session.existing.as_ref() == Some(&target.identity)
                                        && session.display == target.desktop.display
                                        && (session.root_window.is_none()
                                            || session.root_window == Some(target.window_id)))
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
                        windows.sort_by(|a, b| {
                            a["name"]
                                .as_str()
                                .cmp(&b["name"].as_str())
                                .then(a["title"].as_str().cmp(&b["title"].as_str()))
                        });
                        st.running = running;
                        send(
                            &mut out,
                            json!({"type":"window.list","windows":windows,
                            "unavailable_reason":if connected {None} else {Some("desktop_unavailable")}}),
                            "",
                        )?;
                    }
                    Request::Launch {
                        app_id,
                        request_id,
                        backend: _,
                        attach,
                        scale_factor,
                    } => {
                        if lease.is_some() {
                            bail!("detach before launching another session")
                        };
                        let session = launch(
                            &mut state.lock().unwrap(),
                            &app_id,
                            &request_id,
                            scale_factor,
                        )?;
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
                    Request::Resume { session_id } => {
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
                            let windows = if let Some(b) = s.backend.as_mut() {
                                b.scan()?
                            } else {
                                vec![]
                            };
                            known = windows.iter().cloned().map(|w| (w.window_id, w)).collect();
                            let values: Vec<_> = windows
                                .iter()
                                .map(|w| win_value(w, "window.created"))
                                .collect();
                            send(
                                &mut out,
                                json!({"type":"session.snapshot","windows":values}),
                                &s.id,
                            )?;
                        }
                        lease = Some(new);
                    }
                    Request::Detach {} => {
                        bail!("detach")
                    }
                    Request::Hello { .. } => bail!("duplicate hello"),
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
                            Request::Text { window_id, text }
                            | Request::Paste { window_id, text } => {
                                b.commit_text(window_id, text)?
                            }
                            Request::Copy {
                                window_id,
                                request_id,
                            } => b.copy_text(window_id, request_id)?,
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
                    json!({"type":"error","code":code,"message":message}),
                    &sid,
                )?;
            }
        }
        if last_scan.elapsed() < Duration::from_millis(33)
            && !(capture_pending && last_capture.elapsed() >= Duration::from_millis(33))
        {
            continue;
        }
        last_scan = Instant::now();
        if let Some(l) = &lease {
            let mut s = l.0.lock().unwrap();
            let sid = s.id.clone();
            let exited = s.has_exited()?;
            if exited {
                if !exit_notified {
                    for mut c in s.children.drain(..).rev() {
                        stop_child(&mut c);
                    }
                    send(&mut out, json!({"type":"session.exited"}), &sid)?;
                    exit_notified = true;
                }
                continue;
            }
            if let Some(b) = s.backend.as_mut() {
                let windows = b.scan()?;
                for id in known.keys().filter(|id| !b.windows.contains_key(id)) {
                    send(
                        &mut out,
                        json!({"type":"window.destroyed","window_id":id}),
                        &sid,
                    )?;
                }
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
                            ),
                            &sid,
                        )?;
                    }
                    if !w.visible || !b.capture_needed(w.window_id) {
                        continue;
                    }
                    // Metadata-only scans must not postpone the first repaint
                    // after idle. Failed attempts still retain the 30fps bound.
                    last_capture = last_scan;
                    if let Ok(image) = b.capture(w) {
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
