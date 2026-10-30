//! One UI process per paired device/application; per-session lifecycle over a
//! private local socket. No video or controller events go through this broker.
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

pub struct Attachment {
    pub pid: u32,
    pub session: String,
    stream: UnixStream,
    path: PathBuf,
    group: String,
}

fn directory() -> io::Result<PathBuf> {
    let path = PathBuf::from(format!("/tmp/pf-app-{}", unsafe { libc::geteuid() }));
    match fs::DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(path)
}
use std::os::unix::fs::DirBuilderExt;

fn connect(path: &Path) -> io::Result<UnixStream> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let stream = UnixStream::connect(path)?;
    // Socket permissions and the authenticated peer must agree.
    let (mut uid, mut gid) = (0, 0);
    use std::os::fd::AsRawFd;
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0
        || uid != unsafe { libc::geteuid() }
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    Ok(stream)
}

fn send(stream: &mut UnixStream, value: &Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    stream.write_all(&bytes)
}
fn receive(stream: &mut UnixStream) -> io::Result<Value> {
    let mut line = Vec::new();
    let mut byte = [0];
    while line.len() < 16 * 1024 {
        stream.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            return serde_json::from_slice(&line).map_err(Into::into);
        }
        line.push(byte[0]);
    }
    Err(io::ErrorKind::InvalidData.into())
}

/// Recover a running session after the launcher itself restarts. A target is a
/// window/launch target, not the application's grouping key.
pub fn lookup(
    group: &str,
    session: Option<&str>,
    target: Option<&str>,
) -> Result<Option<String>, String> {
    let query = || -> io::Result<Option<String>> {
        let path = directory()?.join(format!("{group}.sock"));
        let mut stream = match connect(&path) {
            Ok(stream) => stream,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(None)
            }
            Err(error) => return Err(error),
        };
        send(&mut stream, &json!({"version":1,"group":group,"op":"list"}))?;
        let response = receive(&mut stream)?;
        let sessions = response["sessions"]
            .as_array()
            .ok_or(io::ErrorKind::InvalidData)?;
        Ok(sessions
            .iter()
            .find(|value| {
                session.is_some_and(|id| value["session"] == id)
                    || target.is_some_and(|id| value["target"] == id)
            })
            .and_then(|value| value["session"].as_str())
            .map(str::to_owned))
    };
    query().map_err(|_| "无法读取本机接收会话，请稍后重试。".into())
}

/// The lock serializes startup across threads and launcher processes. A live
/// owner's socket is never unlinked, even if its main loop is temporarily busy.
pub fn attach_or_start(
    group: &str,
    command: &mut Command,
    session: &str,
    target: Option<&str>,
    fps: u8,
    background: bool,
    icon: Option<&str>,
    picture_mode: crate::display_settings::PictureMode,
) -> Result<Attachment, String> {
    attach_inner(
        group,
        command,
        session,
        target,
        fps,
        background,
        icon,
        picture_mode,
    )
    .map_err(|_| format!("无法接入本机应用入口。远端会话 {session} 已保留，可重试连接。"))
}
fn attach_inner(
    group: &str,
    command: &mut Command,
    session: &str,
    target: Option<&str>,
    fps: u8,
    background: bool,
    icon: Option<&str>,
    picture_mode: crate::display_settings::PictureMode,
) -> io::Result<Attachment> {
    if group.len() != 64
        || !group
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let directory = directory()?;
    let path = directory.join(format!("{group}.sock"));
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join(format!("{group}.lock")))?;
    lock.lock()?;
    let mut stream = match connect(&path) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            // Only refused, owned sockets can be stale. Never remove arbitrary files.
            if let Ok(metadata) = fs::symlink_metadata(&path) {
                if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() }
                {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
                fs::remove_file(&path)?;
            }
            command
                .arg("--application-group")
                .arg(group)
                .stdin(if icon.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Some(target) = target {
                command.arg("--source-target").arg(target);
            }
            if icon.is_some() {
                command.arg("--icon-stdin");
            }
            let mut child = command.spawn()?;
            if let (Some(mut input), Some(icon)) = (child.stdin.take(), icon) {
                let _ = input.write_all(icon.as_bytes());
            }
            // Reaping the process is distinct from observing any one session.
            // In particular, no session-close path ever signals this shared PID.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                match connect(&path) {
                    Ok(stream) => break stream,
                    Err(error) if Instant::now() >= deadline => return Err(error),
                    Err(_) => std::thread::sleep(Duration::from_millis(20)),
                }
            }
        }
        Err(error) => return Err(error),
    };
    send(
        &mut stream,
        &json!({"version":1,"group":group,"op":"attach","session":session,"target":target,"fps":fps,"background":background,"picture_mode":picture_mode}),
    )?;
    let response = receive(&mut stream)?;
    let pid = response["pid"]
        .as_u64()
        .filter(|pid| *pid > 0 && *pid <= i32::MAX as u64)
        .ok_or(io::ErrorKind::InvalidData)? as u32;
    if response["version"] != 1 || response["session"] != session {
        return Err(io::ErrorKind::InvalidData.into());
    }
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    Ok(Attachment {
        pid,
        session: session.into(),
        stream,
        path,
        group: group.into(),
    })
}

impl Attachment {
    pub fn watch(mut self, stop: mpsc::Receiver<()>, closed: impl FnOnce() + Send + 'static) {
        std::thread::spawn(move || {
            let mut line = Vec::new();
            let mut buffer = [0; 1024];
            loop {
                if stop.try_recv().is_ok() {
                    if let Ok(mut stream) = connect(&self.path) {
                        let _ = send(
                            &mut stream,
                            &json!({"version":1,"group":self.group,"op":"detach","session":self.session}),
                        );
                    }
                }
                match self.stream.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(size) => {
                        line.extend_from_slice(&buffer[..size]);
                        if line.len() > 4096 {
                            break;
                        }
                        if let Some(end) = line.iter().position(|b| *b == b'\n') {
                            // Any terminal reply or malformed response ends only
                            // this session registration, never sibling sessions.
                            let _ = serde_json::from_slice::<Value>(&line[..end]);
                            break;
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock
                                | io::ErrorKind::TimedOut
                                | io::ErrorKind::Interrupted
                        ) => {}
                    Err(_) => break,
                }
            }
            closed();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        os::unix::{fs::PermissionsExt, net::UnixListener},
    };

    #[test]
    fn shared_pid_has_independent_session_watchers_and_detach_commands() {
        let group = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let path = directory().unwrap().join(format!("{group}.sock"));
        let listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let server = std::thread::spawn(move || {
            let mut watchers = HashMap::new();
            let mut attached = 0;
            while let Ok((mut stream, _)) = listener.accept() {
                let request = receive(&mut stream).unwrap();
                let id = request["session"].as_str().unwrap_or_default().to_owned();
                match request["op"].as_str().unwrap() {
                    "attach" => {
                        assert_eq!(
                            request["picture_mode"],
                            if id == "two" { "source" } else { "adapt" }
                        );
                        send(
                            &mut stream,
                            &json!({"version":1,"pid":std::process::id(),"session":id}),
                        )
                        .unwrap();
                        watchers.insert(id, stream);
                        attached += 1;
                    }
                    "list" => {
                        send(&mut stream, &json!({"version":1,"sessions":watchers.keys().map(|id| json!({"session":id,"target":format!("window-{id}")})).collect::<Vec<_>>()})).unwrap();
                    }
                    "detach" => {
                        let mut watcher = watchers.remove(&id).unwrap();
                        send(
                            &mut watcher,
                            &json!({"version":1,"session":id,"closed":true}),
                        )
                        .unwrap();
                    }
                    _ => unreachable!(),
                }
                if attached == 2 && watchers.is_empty() {
                    break;
                }
            }
        });
        let one = attach_or_start(
            &group,
            &mut Command::new("/usr/bin/false"),
            "one",
            Some("window-one"),
            30,
            false,
            None,
            crate::display_settings::PictureMode::Adapt,
        )
        .unwrap();
        let two = attach_or_start(
            &group,
            &mut Command::new("/usr/bin/false"),
            "two",
            Some("window-two"),
            60,
            true,
            None,
            crate::display_settings::PictureMode::Source,
        )
        .unwrap();
        assert_eq!(one.pid, two.pid);
        assert_eq!(
            lookup(&group, None, Some("window-two")).unwrap().as_deref(),
            Some("two")
        );
        let (stop_one, requests_one) = mpsc::channel();
        let (stop_two, requests_two) = mpsc::channel();
        let (closed_one, done_one) = mpsc::channel();
        let (closed_two, done_two) = mpsc::channel();
        one.watch(requests_one, move || {
            closed_one.send(()).unwrap();
        });
        two.watch(requests_two, move || {
            closed_two.send(()).unwrap();
        });
        stop_one.send(()).unwrap();
        done_one.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(
            done_two.try_recv().is_err(),
            "closing one session must leave the sibling registered"
        );
        assert_eq!(
            lookup(&group, Some("two"), None).unwrap().as_deref(),
            Some("two")
        );
        stop_two.send(()).unwrap();
        done_two.recv_timeout(Duration::from_secs(3)).unwrap();
        server.join().unwrap();
        fs::remove_file(path).unwrap();
        fs::remove_file(directory().unwrap().join(format!("{group}.lock"))).unwrap();
    }

    #[test]
    #[ignore = "requires a packaged Mac host and private bridge fixture; run application_group_packaged.py"]
    fn packaged_owner_reuses_application_and_detaches_sessions_independently() {
        let source = PathBuf::from(std::env::var_os("PF_GROUP_HOST").unwrap());
        let fixture = PathBuf::from(std::env::var_os("PF_GROUP_FIXTURE").unwrap());
        let endpoint = json!({"device": uuid::Uuid::new_v4().to_string()});
        let group = crate::window_host::application_identity(&endpoint, "group-fixture").unwrap();
        let executable = crate::window_host::prepare(
            &fixture.join("bundles"),
            &source,
            &endpoint,
            "group-fixture",
            Some("PaneFerry 分组验收"),
        )
        .unwrap();
        let command = || {
            let mut command = Command::new(&executable);
            command
                .args([
                    "--native-peer",
                    "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                    "--native-state",
                ])
                .arg(&fixture)
                .arg("--native-helper")
                .arg(fixture.join("bridge"))
                .args(["--session", "one", "--fps", "30"]);
            command
        };
        let one = attach_or_start(
            &group,
            &mut command(),
            "one",
            Some("window-one"),
            30,
            false,
            None,
            crate::display_settings::PictureMode::Adapt,
        )
        .unwrap();
        let two = attach_or_start(
            &group,
            &mut Command::new("/usr/bin/false"),
            "two",
            Some("window-two"),
            60,
            true,
            None,
            crate::display_settings::PictureMode::Adapt,
        )
        .unwrap();
        assert_eq!(one.pid, two.pid);
        let pid = one.pid;
        let (stop_one, requests_one) = mpsc::channel();
        let (stop_two, requests_two) = mpsc::channel();
        let (closed_one, done_one) = mpsc::channel();
        let (closed_two, done_two) = mpsc::channel();
        one.watch(requests_one, move || {
            let _ = closed_one.send(());
        });
        two.watch(requests_two, move || {
            let _ = closed_two.send(());
        });
        let wait_for = |name: &str| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !fixture.join(name).exists() {
                assert!(Instant::now() < deadline, "fixture {name} not ready");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        wait_for("connections-one");
        wait_for("connections-two");
        assert_eq!(
            lookup(&group, None, Some("window-two")).unwrap().as_deref(),
            Some("two")
        );
        fs::write(
            fixture.join("owner.json"),
            serde_json::to_vec(&json!({"pid":pid,"group":group,"executable":executable})).unwrap(),
        )
        .unwrap();
        // The outer driver inspects the real LaunchServices/Dock registration.
        wait_for("inspection-complete");
        stop_one.send(()).unwrap();
        done_one.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(done_two.try_recv().is_err());
        assert!(lookup(&group, Some("two"), None).unwrap().is_some());
        let three = attach_or_start(
            &group,
            &mut Command::new("/usr/bin/false"),
            "three",
            Some("window-three"),
            30,
            true,
            None,
            crate::display_settings::PictureMode::Adapt,
        )
        .unwrap();
        assert_eq!(three.pid, pid);
        let (_stop_three, requests_three) = mpsc::channel();
        let (closed_three, done_three) = mpsc::channel();
        three.watch(requests_three, move || {
            let _ = closed_three.send(());
        });
        wait_for("connections-three");
        // SIGTERM follows precisely the same AppKit quit handler as Cmd-Q.
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        done_two.recv_timeout(Duration::from_secs(5)).unwrap();
        done_three.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(stop_two);
        let deadline = Instant::now() + Duration::from_secs(5);
        while unsafe { libc::kill(pid as i32, 0) } == 0 {
            assert!(Instant::now() < deadline, "shared owner failed to quit");
            std::thread::sleep(Duration::from_millis(20));
        }
        let log = fs::read_to_string(fixture.join("requests")).unwrap();
        for session in ["one", "two", "three"] {
            assert!(log
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .any(|value| value["type"] == "session.detach"
                    && value["fixture_session"] == session));
        }
        let _ = fs::remove_file(directory().unwrap().join(format!("{group}.sock")));
        let _ = fs::remove_file(directory().unwrap().join(format!("{group}.lock")));
        println!("PASS: actual launcher broker + packaged AppKit owner; one PID for three sessions, scoped close and bounded application quit");
    }
}
