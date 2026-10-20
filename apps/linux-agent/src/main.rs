#[cfg(not(unix))]
compile_error!("Linux agent requires Unix");
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use remoteapp_backend_x11::Backend;
use remoteapp_core::{load_catalog, App};
use remoteapp_protocol::{identifier, read_line_limit, Request, Window};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{BufReader, Read, Write},
    os::unix::{
        fs::PermissionsExt,
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
struct Session {
    id: String,
    app_id: String,
    kind: String,
    display: u16,
    backend: Option<Backend>,
    children: Vec<Child>,
    controller: bool,
    exited: bool,
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
        for mut c in self.children.drain(..).rev() {
            stop_child(&mut c);
        }
    }
}
struct State {
    sessions: HashMap<String, Arc<Mutex<Session>>>,
    requests: HashMap<String, String>,
    apps: Vec<App>,
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
    load_catalog(&home()?.join(".config/lan-remoteapp/apps.json"))
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
        Some("run-approved") => {
            let id = args.get(2).context("app id required")?;
            let app = catalog()?
                .into_iter()
                .find(|a| &a.id == id)
                .context("app not approved")?;
            let status = Command::new(app.executable)
                .args(app.argv)
                .env_remove("WAYLAND_DISPLAY")
                .env("GDK_BACKEND", "x11")
                .env("QT_QPA_PLATFORM", "xcb")
                .status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
        Some("check") => {
            let apps = catalog()?;
            println!("catalog: {} approved applications", apps.len());
            for cmd in ["Xvfb", "openbox", "xauth", "xpra"] {
                let ok = Command::new("/usr/bin/env")
                    .args(["which", cmd])
                    .status()?
                    .success();
                println!("{cmd}: {ok}");
            }
            Ok(())
        }
        _ => {
            bail!("usage: remoteapp-agent serve | bridge | check | run-approved APP_ID")
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
fn launch(st: &mut State, app_id: &str, req: &str, kind: &str) -> Result<Arc<Mutex<Session>>> {
    if !identifier(app_id) || !identifier(req) || !["native", "xpra"].contains(&kind) {
        bail!("invalid launch request")
    }
    if let Some(sid) = st.requests.get(req) {
        let s = st.sessions.get(sid).context("session expired")?.clone();
        {
            let mut s = s.lock().unwrap();
            if s.app_id != app_id || s.kind != kind {
                bail!("request id conflict")
            }
            if s.exited
                || s.children
                    .last_mut()
                    .map(|c| c.try_wait())
                    .transpose()?
                    .flatten()
                    .is_some()
            {
                s.exited = true;
                bail!("session exited; use a new request id to start again")
            }
        }
        return Ok(s);
    }
    for session in st.sessions.values() {
        let mut s = session.lock().unwrap();
        if s.children
            .last_mut()
            .and_then(|c| c.try_wait().ok())
            .flatten()
            .is_some()
        {
            s.exited = true;
        }
    }
    st.sessions.retain(|_, s| !s.lock().unwrap().exited);
    if st.requests.len() >= 4096 {
        bail!("request history limit reached; restart after saving work")
    }
    if st.sessions.len() >= 16 {
        bail!("active session limit reached")
    }
    let app = st
        .apps
        .iter()
        .find(|a| a.id == app_id)
        .cloned()
        .context("app not approved")?;
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
        kind: kind.into(),
        display,
        backend: None,
        children: vec![],
        controller: false,
        exited: false,
    };
    let disp = format!(":{display}");
    let auth = st.runtime.join("Xauthority");
    if kind == "native" {
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
            if let Ok(b) = Backend::connect(&disp) {
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
        s.children.push(
            Command::new("dbus-run-session")
                .arg("--")
                .arg(&app.executable)
                .args(&app.argv)
                .env("DISPLAY", &disp)
                .env_remove("WAYLAND_DISPLAY")
                .env("GDK_BACKEND", "x11")
                .env("QT_QPA_PLATFORM", "xcb")
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("application start failed")?,
        );
    } else {
        // app_id is a bounded ASCII identifier; executable path is a fixed installed helper.
        let child = format!(
            "{} run-approved {}",
            quote(&home()?.join(".local/bin/remoteapp-agent").to_string_lossy()),
            app.id
        );
        s.children.push(
            Command::new("xpra")
                .args([
                    "start",
                    &disp,
                    "--daemon=no",
                    "--exit-with-children=yes",
                    "--exit-with-client=no",
                    "--start-new-commands=no",
                    "--file-transfer=no",
                    "--printing=no",
                    "--open-files=no",
                    "--open-url=no",
                    "--speaker=off",
                    "--microphone=off",
                    "--webcam=no",
                    "--mdns=no",
                    "--clipboard=no",
                    "--start-child",
                ])
                .arg(child)
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("xpra not installed")?,
        );
        let until = Instant::now() + Duration::from_secs(8);
        loop {
            if s.children[0].try_wait()?.is_some() {
                bail!("xpra startup failed")
            };
            if std::path::Path::new(&format!("/tmp/.X11-unix/X{display}")).exists() {
                break;
            }
            if Instant::now() > until {
                bail!("xpra readiness timeout")
            };
            thread::sleep(Duration::from_millis(100));
        }
    }
    let session = Arc::new(Mutex::new(s));
    st.requests.insert(req.into(), id.clone());
    st.sessions.insert(id, session.clone());
    Ok(session)
}
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
struct Lease(Arc<Mutex<Session>>);
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut s) = self.0.lock() {
            s.controller = false;
            if let Some(b) = &s.backend {
                b.release_all();
            }
        }
    }
}
fn acquire(session: Arc<Mutex<Session>>) -> Result<Lease> {
    let mut s = session.lock().unwrap();
    if s.controller {
        bail!("session already controlled")
    };
    s.controller = true;
    drop(s);
    Ok(Lease(session))
}
fn send(out: &mut UnixStream, mut v: Value, seq: &mut u64, sid: &str) -> Result<()> {
    *seq += 1;
    v["version"] = json!(1);
    v["seq"] = json!(*seq);
    v["session_id"] = json!(sid);
    let mut bytes = serde_json::to_vec(&v)?;
    if bytes.len() + 1 > remoteapp_protocol::MAX_LINE {
        bail!("outgoing frame too large")
    };
    bytes.push(b'\n');
    out.write_all(&bytes)?;
    Ok(())
}
fn win_value(w: &Window, kind: &str) -> Value {
    let mut v = serde_json::to_value(w).unwrap();
    v["type"] = json!(kind);
    v
}
fn client(mut out: UnixStream, state: Arc<Mutex<State>>) -> Result<()> {
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
                        }
                        Err(_) => break,
                    }
                }
                _ => break,
            }
        }
    });
    let mut seq = 0;
    match rx.recv_timeout(Duration::from_secs(10))? {
        Request::Hello { version: 1 } => {}
        _ => bail!("hello version 1 required"),
    }
    send(
        &mut out,
        json!({"type":"hello","capabilities":["catalog","xpra","native.png","input.pointer","input.key","session.resume","session.list"]}),
        &mut seq,
        "",
    )?;
    let mut last_capture = Instant::now() - Duration::from_secs(1);
    let mut lease: Option<Lease> = None;
    let mut known: HashMap<u32, Window> = HashMap::new();
    let mut frames: HashMap<u32, Vec<u8>> = HashMap::new();
    while !STOP.load(Ordering::SeqCst) {
        let req = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(r) => Some(r),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(_) => break,
        };
        if let Some(req) = req {
            let result: Result<()> = (|| {
                match req {
                    Request::Sessions {} => {
                        let st = state.lock().unwrap();
                        let mut sessions = Vec::new();
                        for session in st.sessions.values() {
                            let mut s = session.lock().unwrap();
                            if s.exited
                                || s.children
                                    .last_mut()
                                    .map(|c| c.try_wait())
                                    .transpose()?
                                    .flatten()
                                    .is_some()
                            {
                                continue;
                            }
                            sessions.push(json!({"session_id":s.id,"app_id":s.app_id,"backend":s.kind,"display":s.display,"controlled":s.controller}));
                        }
                        sessions.sort_by(|a, b| {
                            a["session_id"].as_str().cmp(&b["session_id"].as_str())
                        });
                        send(
                            &mut out,
                            json!({"type":"session.list","sessions":sessions}),
                            &mut seq,
                            "",
                        )?;
                    }
                    Request::List {} => {
                        let st = state.lock().unwrap();
                        let apps: Vec<_> = st
                            .apps
                            .iter()
                            .map(|a| json!({"id":a.id,"name":a.name,"description":a.description}))
                            .collect();
                        send(
                            &mut out,
                            json!({"type":"app.list","apps":apps}),
                            &mut seq,
                            "",
                        )?;
                    }
                    Request::Launch {
                        app_id,
                        request_id,
                        backend,
                        attach,
                    } => {
                        if lease.is_some() {
                            bail!("detach before launching another session")
                        };
                        let session =
                            launch(&mut state.lock().unwrap(), &app_id, &request_id, &backend)?;
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
                                json!({"type":"session.started","backend":s.kind,"display":s.display}),
                                &mut seq,
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
                                &mut seq,
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
                        let b = s
                            .backend
                            .as_mut()
                            .context("native operation unavailable for xpra")?;
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
                            Request::Text { .. } => {
                                bail!("unsupported: Unicode text commit is not implemented")
                            }
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
                send(
                    &mut out,
                    json!({"type":"error","code":"request_failed","message":e.to_string()}),
                    &mut seq,
                    &sid,
                )?;
            }
        }
        if last_capture.elapsed() < Duration::from_millis(100) {
            continue;
        }
        last_capture = Instant::now();
        if let Some(l) = &lease {
            let mut s = l.0.lock().unwrap();
            let sid = s.id.clone();
            if s.exited {
                continue;
            }
            let exited = s
                .children
                .last_mut()
                .map(|c| c.try_wait())
                .transpose()?
                .flatten()
                .is_some();
            if exited {
                s.exited = true;
                for mut c in s.children.drain(..).rev() {
                    stop_child(&mut c);
                }
                send(&mut out, json!({"type":"session.exited"}), &mut seq, &sid)?;
                continue;
            }
            if let Some(b) = s.backend.as_mut() {
                let windows = b.scan()?;
                for id in known.keys().filter(|id| !b.windows.contains_key(id)) {
                    send(
                        &mut out,
                        json!({"type":"window.destroyed","window_id":id}),
                        &mut seq,
                        &sid,
                    )?;
                    frames.remove(id);
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
                            &mut seq,
                            &sid,
                        )?;
                    }
                    if !w.visible {
                        frames.remove(&w.window_id);
                        continue;
                    }
                    if let Ok(png) = b.capture(w) {
                        if frames.get(&w.window_id) != Some(&png) {
                            send(
                                &mut out,
                                json!({"type":"frame","window_id":w.window_id,"geometry_revision":w.geometry_revision,"width":w.width,"height":w.height,"png":STANDARD.encode(&png)}),
                                &mut seq,
                                &sid,
                            )?;
                            frames.insert(w.window_id, png);
                        }
                    }
                }
                known = windows.into_iter().map(|w| (w.window_id, w)).collect();
            }
        }
    }
    let _ = out.shutdown(std::net::Shutdown::Both);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shell_quote() {
        assert_eq!(quote("a'b"), "'a'\\''b'");
    }
    #[test]
    fn id_entropy() {
        assert_eq!(random_id().unwrap().len(), 32);
        assert_ne!(random_id().unwrap(), random_id().unwrap());
    }
}
