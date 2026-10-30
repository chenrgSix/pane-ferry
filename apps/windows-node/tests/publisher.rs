//! Actual QUIC and RWP state machine, with a synthetic desktop (no OS capture claim).
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use paneferry_backend_windows::{Application, Geometry, Input, Target, VideoFrame, WindowSystem};
use paneferry_network::{
    framing,
    service::{Config, Service},
};
use paneferry_platform::ipc::{self, LocalStream};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::Semaphore,
};

#[derive(Default)]
struct Observed {
    input: Vec<Input>,
    attached: usize,
    detached: usize,
    dead: bool,
    launches: Vec<bool>,
    pending_once: bool,
}
static OBSERVED: OnceLock<Mutex<Observed>> = OnceLock::new();
fn observed() -> &'static Mutex<Observed> {
    OBSERVED.get_or_init(Default::default)
}
struct Fixture {
    attached: bool,
    next: usize,
    frames: Vec<Vec<u8>>,
}
fn target() -> Target {
    Target {
        id: "win-fixture".into(),
        handle: 1,
        pid: 123,
        started: 42,
        generation: 7,
        name: "Windows fixture".into(),
        application_key: "exe-fixture".into(),
        auto_connectable: true,
    }
}
impl WindowSystem for Fixture {
    fn applications(&mut self) -> Result<Vec<Application>> {
        Ok(vec![Application {
            id: "exe-fixture".into(),
            name: "Installed fixture".into(),
            description: "Fixture".into(),
            unavailable_reason: None,
        }])
    }
    fn launch_application(&mut self, id: &str, start: bool) -> Result<Target> {
        ensure!(id == "exe-fixture", "app_not_approved");
        let mut observed = observed().lock().unwrap();
        observed.launches.push(start);
        if observed.pending_once {
            observed.pending_once = false;
            anyhow::bail!("application_window_pending");
        }
        Ok(target())
    }
    fn windows(&mut self) -> Result<Vec<Target>> {
        let mut utility = target();
        utility.id = "win-utility".into();
        utility.auto_connectable = false;
        Ok(vec![target(), utility])
    }
    fn geometry(&mut self, t: &Target) -> Result<Geometry> {
        ensure!(
            !observed().lock().unwrap().dead && t.started == 42,
            "session_exited"
        );
        Ok(Geometry {
            content_revision: 0,
            title: "Windows fixture".into(),
            width: 640,
            height: 480,
            visible: true,
        })
    }
    fn attach(&mut self, t: &Target, _fps: u32) -> Result<()> {
        self.geometry(t)?;
        self.attached = true;
        self.next = 0;
        observed().lock().unwrap().attached += 1;
        Ok(())
    }
    fn keyframe(&mut self, _target: &Target) -> Result<()> {
        Ok(())
    }
    fn frame(&mut self, _: &Target) -> Result<Option<VideoFrame>> {
        if !self.attached || self.next >= self.frames.len() {
            return Ok(None);
        }
        let n = self.next;
        self.next += 1;
        Ok(Some(VideoFrame {
            content_revision: 0,
            data: self.frames[n].clone(),
            width: 640,
            height: 480,
            stream_id: 1,
            key_frame: n == 0,
        }))
    }
    fn input(&mut self, t: &Target, input: Input) -> Result<()> {
        self.geometry(t)?;
        observed().lock().unwrap().input.push(input);
        Ok(())
    }
    fn detach(&mut self) {
        if self.attached {
            observed().lock().unwrap().detached += 1;
            self.attached = false;
        }
    }
}
fn fixture() -> Result<Box<dyn WindowSystem>> {
    let video: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/native-video.json"))?;
    Ok(Box::new(Fixture {
        attached: false,
        next: 0,
        frames: video["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| STANDARD.decode(v.as_str().unwrap()).unwrap())
            .collect(),
    }))
}
async fn send<W: AsyncWrite + Unpin>(write: &mut W, mut value: Value) {
    value["version"] = json!(1);
    framing::json(write, &value).await.unwrap();
}
async fn read<R: AsyncRead + Unpin>(read: &mut BufReader<R>, kind: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value = framing::line(read)
                .await
                .unwrap_or_else(|e| panic!("waiting for {kind}: {e}"));
            if value["type"] == kind {
                return value;
            }
        }
    })
    .await
    .unwrap()
}
async fn connect(path: &std::path::Path) -> LocalStream {
    for _ in 0..100 {
        if let Ok(socket) = LocalStream::connect(path).await {
            return socket;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("route did not become available")
}
async fn handshake(
    socket: LocalStream,
) -> (BufReader<impl AsyncRead + Unpin>, impl AsyncWrite + Unpin) {
    let (read, mut write) = socket.into_split();
    let mut read = BufReader::new(read);
    send(&mut write, json!({"type":"hello"})).await;
    let hello = read_value(&mut read).await;
    assert_eq!(hello["type"], "hello");
    assert!(hello["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!("input.key.logical")));
    (read, write)
}
async fn read_value<R: AsyncRead + Unpin>(read: &mut BufReader<R>) -> Value {
    tokio::time::timeout(Duration::from_secs(5), framing::line(read))
        .await
        .unwrap()
        .unwrap()
}
async fn wait_for(mut predicate: impl FnMut() -> bool) {
    for _ in 0..100 {
        if predicate() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(predicate());
}
fn config(root: &std::path::Path, name: &str, publisher: std::path::PathBuf) -> Config {
    Config {
        state: root.join(name),
        publisher,
        listen: "127.0.0.1:0".parse().unwrap(),
        name: name.into(),
        device_id: None,
        bulk_agent: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn paired_receivers_single_window_input_disconnect_and_same_session_resume() {
    // Short paths keep Unix sockets within the macOS sockaddr_un limit.
    let root = tempfile::Builder::new()
        .prefix("pfw")
        .tempdir_in(if cfg!(unix) {
            std::path::PathBuf::from("/tmp")
        } else {
            std::env::temp_dir()
        })
        .unwrap();
    let publisher = root.path().join("p.sock");
    let listener = ipc::bind(&publisher).unwrap();
    let source = Service::new(config(root.path(), "source", publisher.clone())).unwrap();
    let state = paneferry_windows::State::new(source.id().into(), true);
    let slots = Arc::new(Semaphore::new(16));
    let worker = tokio::spawn(paneferry_windows::serve(
        listener,
        state.clone(),
        false,
        fixture,
        slots.clone(),
    ));
    let source_task = tokio::spawn(source.clone().run());
    let receiver = Service::new(config(
        root.path(),
        "receiver",
        root.path().join("unused.sock"),
    ))
    .unwrap();
    let receiver_task = tokio::spawn(receiver.clone().run());
    receiver
        .pair(&source.invite().unwrap(), &source.address().to_string())
        .await
        .unwrap();
    let route = root
        .path()
        .join("receiver")
        .join(format!("r-{}.sock", source.id()));
    let (mut rd, mut wr) = handshake(connect(&route).await).await;
    send(&mut wr, json!({"type":"sharing.set","enabled":false})).await;
    // QUIC rejects local management and closes this connection.
    assert!(
        tokio::time::timeout(Duration::from_secs(5), framing::line(&mut rd))
            .await
            .unwrap()
            .is_err()
    );
    drop((rd, wr));
    let (mut rd, mut wr) = handshake(connect(&route).await).await;
    send(&mut wr, json!({"type":"window.list"})).await;
    let list = read(&mut rd, "window.list").await;
    assert_eq!(list["windows"][0]["id"], "win-fixture");
    // Exercise the same transport decoder used by the Mac takeover planner.
    let windows = paneferry_transport::desktop_windows(&paneferry_transport::Target {
        socket: route.clone(),
    })
    .await
    .unwrap();
    assert_eq!(
        windows.windows[0].application_key.as_deref(),
        Some("exe-fixture")
    );
    assert!(windows.windows[0].auto_connectable);
    assert_eq!(
        windows.windows[1].application_key,
        windows.windows[0].application_key
    );
    assert!(!windows.windows[1].auto_connectable);
    send(&mut wr,json!({"type":"app.launch","app_id":"not-approved","request_id":"bad","backend":"native","attach":false})).await;
    assert_eq!(read(&mut rd, "error").await["code"], "app_not_approved");
    let catalog = paneferry_transport::catalog(&paneferry_transport::Target {
        socket: route.clone(),
    })
    .await
    .unwrap();
    assert_eq!(catalog.apps[0].id, "exe-fixture");
    assert_eq!(catalog.apps[0].source, "desktop");
    assert!(catalog.discovery_error.is_none());
    observed().lock().unwrap().pending_once = true;
    let app_launch = json!({"type":"app.launch","app_id":"exe-fixture","request_id":"installed-request","backend":"native","attach":false});
    send(&mut wr, app_launch.clone()).await;
    assert_eq!(
        read(&mut rd, "error").await["code"],
        "application_window_pending"
    );
    send(&mut wr, json!({"type":"app.launch","app_id":"win-fixture","request_id":"installed-request","backend":"native","attach":false})).await;
    assert_eq!(read(&mut rd, "error").await["code"], "request_id_conflict");
    send(&mut wr, app_launch.clone()).await;
    let app_session = read(&mut rd, "session.started").await["session_id"].clone();
    send(&mut wr, app_launch).await;
    assert_eq!(
        read(&mut rd, "session.started").await["session_id"],
        app_session
    );
    assert_eq!(observed().lock().unwrap().launches, [true, false]);
    let launch = json!({"type":"app.launch","app_id":"win-fixture","request_id":"request-1","backend":"native","attach":false});
    send(&mut wr, launch.clone()).await;
    let started = read(&mut rd, "session.started").await;
    let session = started["session_id"].as_str().unwrap().to_owned();
    send(&mut wr, launch).await;
    assert_eq!(
        read(&mut rd, "session.started").await["session_id"],
        session
    );
    send(
        &mut wr,
        json!({"type":"video.enable","codec":"h264","max_fps":60}),
    )
    .await;
    assert_eq!(read(&mut rd, "video.ready").await["max_fps"], 30);
    send(
        &mut wr,
        json!({"type":"session.resume","session_id":session}),
    )
    .await;
    let snapshot = read(&mut rd, "session.snapshot").await;
    assert_eq!(snapshot["windows"][0]["window_id"], 1);
    let frame = read(&mut rd, "frame").await;
    assert_eq!(frame["key_frame"], true);
    assert_eq!(
        frame["geometry_revision"],
        snapshot["windows"][0]["geometry_revision"]
    );
    let pframe = read(&mut rd, "frame").await;
    assert_eq!(pframe["key_frame"], false);
    assert!(pframe["seq"].as_u64() > frame["seq"].as_u64());
    let (mut second_read, mut second_write) = handshake(connect(&route).await).await;
    send(
        &mut second_write,
        json!({"type":"video.enable","codec":"h264"}),
    )
    .await;
    read(&mut second_read, "video.ready").await;
    send(
        &mut second_write,
        json!({"type":"session.resume","session_id":session}),
    )
    .await;
    assert_eq!(
        read(&mut second_read, "error").await["code"],
        "session_controlled"
    );
    // Both receiver dialects, plus committed Chinese and a supplementary character.
    send(
        &mut wr,
        json!({"type":"input.key","window_id":1,"keysym":65507,"pressed":true}),
    )
    .await;
    send(
        &mut wr,
        json!({"type":"input.key.logical","window_id":1,"key":"Control_L","pressed":false}),
    )
    .await;
    send(
        &mut wr,
        json!({"type":"input.text.commit","window_id":1,"text":"中文输入🙂"}),
    )
    .await;
    send(
        &mut wr,
        json!({"type":"input.pointer","window_id":1,"x":12,"y":34,"button":1,"pressed":true}),
    )
    .await;
    send(
        &mut wr,
        json!({"type":"input.key","window_id":99,"keysym":65,"pressed":true}),
    )
    .await;
    assert_eq!(read(&mut rd, "error").await["code"], "window_unavailable");
    send(
        &mut wr,
        json!({"type":"input.text.commit","window_id":1,"text":"bad","command":"cmd.exe"}),
    )
    .await;
    assert_eq!(read(&mut rd, "error").await["code"], "invalid_request");
    wait_for(|| observed().lock().unwrap().input.len() == 4).await;
    assert!(observed()
        .lock()
        .unwrap()
        .input
        .contains(&Input::Text("中文输入🙂".into())));
    assert_eq!(
        observed().lock().unwrap().input[0],
        Input::Key {
            virtual_key: 0xa2,
            pressed: true
        }
    );
    // Abrupt socket loss releases the controller without ending the source application.
    wr.shutdown().await.unwrap();
    drop((rd, wr));
    wait_for(|| observed().lock().unwrap().detached == 1).await;
    send(
        &mut second_write,
        json!({"type":"session.resume","session_id":session}),
    )
    .await;
    assert_eq!(
        read(&mut second_read, "session.snapshot").await["session_id"],
        session
    );
    assert_eq!(read(&mut second_read, "frame").await["key_frame"], true);
    assert_eq!(observed().lock().unwrap().input.len(), 4); // No replay after reconnect.
    observed().lock().unwrap().dead = true;
    assert_eq!(
        read(&mut second_read, "session.exited").await["session_id"],
        session
    );
    drop((second_read, second_write));
    state.lock().unwrap().stop();
    let _ = tokio::time::timeout(Duration::from_secs(5), slots.acquire_many_owned(16))
        .await
        .unwrap();
    worker.abort();
    source_task.abort();
    receiver_task.abort();
}
