mod request;
mod video_flow;
use anyhow::{bail, ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use paneferry_backend_windows::{Geometry, Target, WindowSystem};
use paneferry_network::framing;
use paneferry_platform::ipc::{self, LocalListener, LocalStream};
use request::Request;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncWrite, BufReader},
    sync::Semaphore,
};

struct Session {
    app_id: String,
    target: Target,
    controller: Option<uuid::Uuid>,
}
pub struct State {
    device_id: String,
    sharing: bool,
    stopping: bool,
    approved: HashMap<String, Target>,
    sessions: HashMap<String, Session>,
    requests: HashMap<String, (String, String)>,
    launch_attempts: HashMap<String, String>,
}
impl State {
    pub fn stop(&mut self) {
        self.sharing = false;
        self.stopping = true;
    }
    pub fn new(device_id: String, sharing: bool) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            device_id,
            sharing,
            stopping: false,
            approved: HashMap::new(),
            sessions: HashMap::new(),
            requests: HashMap::new(),
            launch_attempts: HashMap::new(),
        }))
    }
}
fn prior_request<'a>(state: &'a State, request: &str, app: &str) -> Result<Option<&'a str>> {
    match state.requests.get(request) {
        Some((original_app, session)) => {
            ensure!(original_app == app, "request_id_conflict");
            Ok(Some(session))
        }
        None => Ok(None),
    }
}

fn release_controller(state: &mut State, id: &str, owner: uuid::Uuid, exited: bool) {
    if let Some(session) = state
        .sessions
        .get_mut(id)
        .filter(|s| s.controller == Some(owner))
    {
        session.controller = None;
        if exited {
            state.sessions.remove(id);
        }
    }
}

pub type Factory = fn() -> Result<Box<dyn WindowSystem>>;
pub async fn serve(
    listener: LocalListener,
    state: Arc<Mutex<State>>,
    local: bool,
    factory: Factory,
    slots: Arc<Semaphore>,
) -> Result<()> {
    loop {
        let (socket, _) = listener.accept().await?;
        if !ipc::same_user(&socket) {
            continue;
        }
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let state = state.clone();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if let Ok(system) = factory() {
                let _ = runtime.block_on(client(socket, state, local, system));
            }
        });
    }
}
struct Client {
    id: uuid::Uuid,
    state: Arc<Mutex<State>>,
    system: Box<dyn WindowSystem>,
    session: Option<(String, Target)>,
    video: bool,
    color: paneferry_backend_windows::color::ColorMode,
    fps: u32,
    flow: video_flow::VideoFlow,
    sequence: u64,
    geometry: Option<Geometry>,
    revision: u64,
    video_epoch: u64,
}
impl Drop for Client {
    fn drop(&mut self) {
        self.system.detach();
        if let Some((id, _)) = &self.session {
            if let Some(s) = self.state.lock().unwrap().sessions.get_mut(id) {
                if s.controller == Some(self.id) {
                    s.controller = None;
                }
            }
        }
    }
}
impl Client {
    fn shared(&self) -> Result<()> {
        ensure!(self.state.lock().unwrap().sharing, "sharing_disabled");
        Ok(())
    }
    fn retire_exited(&mut self) {
        let sessions: Vec<_> = self
            .state
            .lock()
            .unwrap()
            .sessions
            .iter()
            .map(|(id, s)| (id.clone(), s.target.clone()))
            .collect();
        for (id, target) in sessions {
            if self
                .system
                .geometry(&target)
                .is_err_and(|error| public_error(&error) == "session_exited")
            {
                // Retain request tombstones: retrying an old launch cannot acquire a new window.
                self.state.lock().unwrap().sessions.remove(&id);
            }
        }
    }
    fn metadata(&self) -> Value {
        let g = self.geometry.as_ref().unwrap();
        let desktop = self.session.as_ref().is_some_and(|s| s.1.monitor.is_some());
        json!({"window_id":1,"parent_id":null,"title":g.title,"role":if desktop {"desktop"} else {"toplevel"},"surface_type":if desktop {"desktop"} else {"window"},"x":0,"y":0,"width":g.width,"height":g.height,"visible":g.visible,"scale_factor":1,"geometry_revision":self.revision})
    }
    async fn send<W: AsyncWrite + Unpin>(&mut self, write: &mut W, mut value: Value) -> Result<()> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("sequence exhausted"))?;
        value["version"] = json!(1);
        value["seq"] = json!(self.sequence);
        if value.get("session_id").is_none() {
            value["session_id"] = json!(self.session.as_ref().map(|s| s.0.as_str()).unwrap_or(""));
        }
        let _timing = paneferry_backend_windows::metrics::span("publisher.ipc_send");
        tokio::time::timeout(Duration::from_secs(5), framing::json(write, &value)).await??;
        Ok(())
    }
    fn attach(&mut self, id: &str) -> Result<()> {
        self.shared()?;
        ensure!(self.video && self.session.is_none(), "invalid_request");
        let target = {
            let mut state = self.state.lock().unwrap();
            let session = state
                .sessions
                .get_mut(id)
                .ok_or_else(|| anyhow::anyhow!("session_expired"))?;
            ensure!(session.controller.is_none(), "session_controlled");
            session.controller = Some(self.id);
            session.target.clone()
        };
        let result = (|| {
            self.system.attach(&target, self.fps)?;
            let geometry = self.system.geometry(&target)?;
            self.geometry = Some(geometry);
            Ok(())
        })();
        if let Err(error) = result {
            release_controller(
                &mut self.state.lock().unwrap(),
                id,
                self.id,
                public_error(&error) == "session_exited",
            );
            self.system.detach();
            return Err(error);
        }
        if !self
            .state
            .lock()
            .unwrap()
            .sessions
            .get(id)
            .is_some_and(|s| s.controller == Some(self.id))
        {
            self.system.detach();
            bail!("session_expired");
        }
        self.session = Some((id.into(), target));
        self.revision = 1;
        Ok(())
    }
    async fn handle<W: AsyncWrite + Unpin>(
        &mut self,
        request: Request,
        write: &mut W,
        local: bool,
    ) -> Result<bool> {
        match request {
            Request::Info => {
                let value = {
                    let state = self.state.lock().unwrap();
                    json!({"type":"node.info","device_id":state.device_id,"name":std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows".into()),"platform":"windows","publishing_supported":true,"receiving_supported":true,"sharing_enabled":state.sharing,"session_count":state.sessions.len()})
                };
                self.send(write, value).await?;
            }
            Request::Sharing { enabled } => {
                ensure!(local, "invalid_request");
                self.state.lock().unwrap().sharing = enabled;
                self.send(write, json!({"type":"sharing.changed","enabled":enabled}))
                    .await?;
            }
            Request::Apps => {
                self.shared()?;
                let (apps, error) = match self.system.applications() {
                    Ok(apps) => (apps.into_iter().take(512).map(|a| json!({
                        "id":a.id,"name":a.name,"description":a.description,
                        "source":"desktop","icon_png":null,"unavailable_reason":a.unavailable_reason
                    })).collect::<Vec<_>>(), None),
                    Err(error) => {
                        eprintln!("PaneFerry Windows application discovery: {error:#}");
                        (vec![], Some("windows_application_discovery_unavailable"))
                    }
                };
                self.send(write,json!({"type":"app.list","apps":apps,"discovery_supported":true,"discovery_error":error})).await?;
            }
            Request::Desktops => {
                self.shared()?;
                let monitors = self.system.monitors()?;
                self.send(
                    write,
                    json!({"type":"desktop.list","monitors":monitors,"unavailable_reason":null}),
                )
                .await?;
            }
            Request::StartDesktop {
                monitor_id,
                request_id,
            } => {
                self.shared()?;
                self.retire_exited();
                ensure!(
                    paneferry_protocol::identifier(&monitor_id)
                        && paneferry_protocol::identifier(&request_id),
                    "invalid_request"
                );
                // Resolve only an enumerated monitor. Never accept a remote HWND,
                // shell window, executable path or app.launch alias as a desktop.
                let target = self.system.monitor_target(&monitor_id)?;
                ensure!(
                    target.monitor.as_deref() == Some(monitor_id.as_str()),
                    "desktop_monitor_unavailable"
                );
                self.system.geometry(&target)?;
                let app_id = format!("desktop-{monitor_id}");
                let id = {
                    let mut state = self.state.lock().unwrap();
                    ensure!(state.sharing, "sharing_disabled");
                    ensure!(
                        !state.launch_attempts.contains_key(&request_id),
                        "request_id_conflict"
                    );
                    if let Some(id) = prior_request(&state, &request_id, &app_id)? {
                        ensure!(state.sessions.contains_key(id), "session_exited");
                        id.to_owned()
                    } else {
                        ensure!(
                            state.requests.len() + state.launch_attempts.len() < 1024,
                            "request_history_limit"
                        );
                        let id = if let Some((id, _)) = state
                            .sessions
                            .iter()
                            .find(|(_, s)| s.target.id == target.id)
                        {
                            id.clone()
                        } else {
                            ensure!(state.sessions.len() < 16, "session_limit");
                            let id = uuid::Uuid::new_v4().to_string();
                            state.sessions.insert(
                                id.clone(),
                                Session {
                                    app_id: app_id.clone(),
                                    target,
                                    controller: None,
                                },
                            );
                            id
                        };
                        state.requests.insert(request_id, (app_id, id.clone()));
                        id
                    }
                };
                self.send(write,json!({"type":"session.started","session_id":id,"backend":"native","display":0,"scale_factor":1,"mode":"desktop","monitor_id":monitor_id})).await?;
            }
            Request::Windows => {
                self.shared()?;
                self.retire_exited();
                let targets = self.system.windows()?;
                let mut windows = Vec::new();
                let mut approved = HashMap::new();
                for target in targets.into_iter().take(128) {
                    let Ok(g) = self.system.geometry(&target) else {
                        continue;
                    };
                    if !g.visible {
                        continue;
                    }
                    let state = self.state.lock().unwrap();
                    let existing = state
                        .sessions
                        .iter()
                        .find(|(_, s)| s.target.id == target.id);
                    windows.push(json!({"id":target.id,"window_id":windows.len()+1,"name":target.name,"title":g.title,"icon_png":null,"minimized":false,"controlled":existing.is_some_and(|(_,s)|s.controller.is_some()),"session_id":existing.map(|(id,_)|id),"application_key":target.application_key,"auto_connectable":target.auto_connectable}));
                    approved.insert(target.id.clone(), target);
                }
                self.state.lock().unwrap().approved = approved;
                self.send(
                    write,
                    json!({"type":"window.list","windows":windows,"unavailable_reason":null}),
                )
                .await?;
            }
            Request::Sessions => {
                self.shared()?;
                self.retire_exited();
                let sessions: Vec<_> = {
                    let s = self.state.lock().unwrap();
                    s.sessions
                        .iter()
                        .map(|(id, s)| {
                            (
                                id.clone(),
                                s.app_id.clone(),
                                s.target.clone(),
                                s.controller.is_some(),
                            )
                        })
                        .collect()
                };
                let mut values = Vec::new();
                for (id, app_id, target, controlled) in sessions {
                    if let Ok(g) = self.system.geometry(&target) {
                        values.push(json!({"session_id":id,"app_id":app_id,"backend":"native","display":0,"controlled":controlled,"window_count":if g.visible{1}else{0},"mode":if target.monitor.is_some(){"desktop"}else{"app"},"monitor_id":target.monitor}));
                    }
                }
                self.send(write, json!({"type":"session.list","sessions":values}))
                    .await?;
            }
            Request::KeyFrame { window_id } => {
                eprintln!("PF_VIDEO keyframe_reason=request");
                self.video_epoch = self.video_epoch.wrapping_add(1);
                self.shared()?;
                ensure!(window_id == 1, "window_unavailable");
                let (_, target) = self
                    .session
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("session_expired"))?;
                self.system.keyframe(target)?;
            }
            Request::VideoConfigure { max_fps } => {
                ensure!(
                    self.video && (1..=paneferry_protocol::MAX_VIDEO_FPS as u32).contains(&max_fps),
                    "invalid_request"
                );
                if self.fps != max_fps {
                    self.video_epoch = self.video_epoch.wrapping_add(1);
                }
                self.system.set_video_fps(max_fps)?;
                self.fps = max_fps;
            }
            Request::VideoFeedback { frame_id, timing } => {
                if let Some(quality) = self.flow.acknowledge(frame_id, timing) {
                    self.system.set_video_quality(quality)?;
                }
            }
            Request::Video {
                codec,
                color,
                max_fps,
                feedback,
            } => {
                ensure!(
                    self.session.is_none() && codec == "h264" && (1..=90).contains(&max_fps),
                    "invalid_request"
                );
                self.system.set_video_color(color)?;
                self.color = color;
                self.flow.enable(feedback);
                self.video = true;
                self.fps = max_fps.min(paneferry_protocol::MAX_VIDEO_FPS as u32);
                self.send(
                    write,
                    json!({"type":"video.ready","codec":"h264","color":self.color,"max_fps":self.fps}),
                )
                .await?;
            }
            Request::Launch {
                app_id,
                request_id,
                backend,
                attach,
                scale_factor,
            } => {
                self.shared()?;
                ensure!(
                    paneferry_protocol::identifier(&app_id)
                        && paneferry_protocol::identifier(&request_id)
                        && backend == "native"
                        && (1..=2).contains(&scale_factor),
                    "invalid_request"
                );
                if attach {
                    ensure!(self.video && self.session.is_none(), "invalid_request");
                }
                let existing_target = {
                    let s = self.state.lock().unwrap();
                    if let Some(original) = s.launch_attempts.get(&request_id) {
                        ensure!(original == &app_id, "request_id_conflict");
                    }
                    if let Some(id) = prior_request(&s, &request_id, &app_id)? {
                        Some(
                            s.sessions
                                .get(id)
                                .ok_or_else(|| anyhow::anyhow!("session_exited"))?
                                .target
                                .clone(),
                        )
                    } else {
                        s.approved.get(&app_id).cloned()
                    }
                };
                let target = if let Some(target) = existing_target {
                    target
                } else {
                    // Re-resolve catalog IDs locally even when a restored client
                    // skips app.list. No remote path, command or argv is accepted.
                    ensure!(
                        self.system
                            .applications()?
                            .iter()
                            .any(|a| a.id == app_id && a.unavailable_reason.is_none()),
                        "app_not_approved"
                    );
                    let start = {
                        let mut s = self.state.lock().unwrap();
                        ensure!(s.sharing, "sharing_disabled");
                        ensure!(s.sessions.len() < 16, "session_limit");
                        if let Some(original) = s.launch_attempts.get(&request_id) {
                            ensure!(original == &app_id, "request_id_conflict");
                            false
                        } else {
                            ensure!(
                                s.requests.len() + s.launch_attempts.len() < 1024,
                                "request_history_limit"
                            );
                            // Reserve before spawning. Concurrent retries may
                            // wait for its window, but cannot launch it twice.
                            s.launch_attempts.insert(request_id.clone(), app_id.clone());
                            true
                        }
                    };
                    self.system.launch_application(&app_id, start)?
                };
                self.shared()?;
                self.system.geometry(&target)?; // Recheck process creation time and current user before acquiring.
                let id = {
                    let mut s = self.state.lock().unwrap();
                    // Another client can claim this request while geometry is
                    // checked outside the mutex. Revalidate its app binding.
                    ensure!(s.sharing, "sharing_disabled");
                    if let Some(original) = s.launch_attempts.get(&request_id) {
                        ensure!(original == &app_id, "request_id_conflict");
                    }
                    if let Some(id) = prior_request(&s, &request_id, &app_id)? {
                        id.to_owned()
                    } else {
                        ensure!(
                            s.requests.len() + s.launch_attempts.len()
                                - usize::from(s.launch_attempts.contains_key(&request_id))
                                < 1024,
                            "request_history_limit"
                        );
                        let old = s
                            .sessions
                            .iter()
                            .find(|(_, v)| v.target.id == target.id)
                            .map(|(id, _)| id.clone());
                        let id = if let Some(id) = old {
                            id
                        } else {
                            ensure!(s.sessions.len() < 16, "session_limit");
                            let id = uuid::Uuid::new_v4().to_string();
                            s.sessions.insert(
                                id.clone(),
                                Session {
                                    app_id: app_id.clone(),
                                    target,
                                    controller: None,
                                },
                            );
                            id
                        };
                        s.launch_attempts.remove(&request_id);
                        s.requests.insert(request_id, (app_id, id.clone()));
                        id
                    }
                };
                self.send(write,json!({"type":"session.started","session_id":id,"backend":"native","display":0,"scale_factor":1})).await?;
                if attach {
                    self.attach(&id)?;
                    let mut value = self.metadata();
                    value["type"] = json!("window.created");
                    self.send(write, value).await?;
                }
            }
            Request::Resume { session_id } => {
                ensure!(
                    paneferry_protocol::identifier(&session_id),
                    "invalid_request"
                );
                self.attach(&session_id)?;
                self.send(
                    write,
                    json!({"type":"session.snapshot","windows":[self.metadata()]}),
                )
                .await?;
            }
            Request::Detach => return Ok(true),
            other => {
                self.shared()?;
                let (window, input) = other.input()?;
                ensure!(window == 1, "window_unavailable");
                let target = &self
                    .session
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("window_unavailable"))?
                    .1;
                if let Err(error) = self.system.input(target, input) {
                    // Static failure stages only: do not log keys, text, pointer
                    // positions, window titles or incoming request payloads.
                    eprintln!("PaneFerry Windows input error: {error:#}");
                    return Err(error);
                }
            }
        }
        Ok(false)
    }
    #[cfg(test)]
    async fn tick<W: AsyncWrite + Unpin>(&mut self, write: &mut W) -> Result<()> {
        if let Some(task) = self.begin_tick(write).await? {
            self.finish_tick(write, task.await?).await?;
        }
        Ok(())
    }
    async fn begin_tick<W: AsyncWrite + Unpin>(
        &mut self,
        write: &mut W,
    ) -> Result<Option<paneferry_backend_windows::FrameTask>> {
        self.shared()?;
        let target = self.session.as_ref().unwrap().1.clone();
        let geometry = self.system.geometry(&target)?;
        self.publish_geometry(write, geometry).await?;
        // Stop producing before the encoder when the controller falls behind.
        // Encoded P-frames must never be dropped independently of their references.
        match self.flow.ready(std::time::Instant::now()) {
            video_flow::Ready::Wait => return Ok(None),
            video_flow::Ready::Restart => {
                eprintln!("PF_VIDEO keyframe_reason=feedback_timeout");
                self.system.keyframe(&target)?;
            }
            video_flow::Ready::Send => {}
        }
        Ok(Some(self.system.begin_frame(&target)?))
    }
    async fn publish_geometry<W: AsyncWrite + Unpin>(
        &mut self,
        write: &mut W,
        geometry: Geometry,
    ) -> Result<()> {
        // Layout inside the composed canvas is video content, not a wire
        // geometry change. Advancing the wire revision would discard queued
        // reference frames in the network receiver and reset the video stream.
        let changed = self.geometry.as_ref().is_none_or(|g| {
            g.width != geometry.width
                || g.height != geometry.height
                || g.visible != geometry.visible
                || g.title != geometry.title
        });
        if changed {
            if self.geometry.as_ref().is_none_or(|g| {
                g.width != geometry.width
                    || g.height != geometry.height
                    || g.visible != geometry.visible
            }) {
                self.revision += 1;
            }
            self.geometry = Some(geometry);
            let mut value = self.metadata();
            value["type"] = json!("window.metadata");
            self.send(write, value).await?;
        } else {
            self.geometry = Some(geometry);
        }
        Ok(())
    }
    async fn finish_tick<W: AsyncWrite + Unpin>(
        &mut self,
        write: &mut W,
        frame: Option<paneferry_backend_windows::VideoFrame>,
    ) -> Result<()> {
        self.shared()?;
        let target = self.session.as_ref().unwrap().1.clone();
        if let Some(frame) = frame {
            // The worker publishes the layout used by this capture before it
            // completes the task. Compare against that snapshot, not the one
            // read before capture (which necessarily predates a new popup).
            let geometry = self.system.geometry(&target)?;
            let dimensions_changed = self.geometry.as_ref().is_none_or(|g| {
                g.width != geometry.width || g.height != geometry.height || !g.visible
            });
            if frame.width != geometry.width
                || frame.height != geometry.height
                || !geometry.visible
                || frame.content_revision != geometry.content_revision
                || (dimensions_changed && !frame.key_frame)
            {
                // The source window may resize after the metadata snapshot but
                // before capture. Do not disconnect or label this frame with an
                // obsolete revision. Dropping an encoded access unit also drops
                // a decoder reference, so restart the stream before trying again.
                eprintln!("PF_VIDEO keyframe_reason=geometry_race");
                self.system.keyframe(&target)?;
                return Ok(());
            }
            self.publish_geometry(write, geometry).await?;
            ensure!(frame.data.len() <= 12 * 1024 * 1024, "capture_failed");
            let frame_id = self.flow.sent(std::time::Instant::now());
            self.send(write,json!({"type":"frame","window_id":1,"geometry_revision":self.revision,"codec":"h264","width":frame.width,"height":frame.height,"color":self.color,"encoded_width":self.color.dimensions(frame.width,frame.height)?.0,"encoded_height":self.color.dimensions(frame.width,frame.height)?.1,"stream_id":frame.stream_id,"key_frame":frame.key_frame,"frame_id":frame_id,"data":STANDARD.encode(frame.data)})).await?;
        }
        Ok(())
    }
}
fn public_error(error: &anyhow::Error) -> &'static str {
    match error.root_cause().to_string().as_str() {
        "sharing_disabled" => "sharing_disabled",
        "desktop_locked" => "desktop_locked",
        "session_exited" => "session_exited",
        "session_expired" => "session_expired",
        "session_controlled" => "session_controlled",
        "session_limit" => "session_limit",
        "request_history_limit" => "request_history_limit",
        "request_id_conflict" => "request_id_conflict",
        "app_not_approved" => "app_not_approved",
        "application_window_pending" => "application_window_pending",
        "application_start_failed" => "application_start_failed",
        "application_multiple_windows" => "application_multiple_windows",
        "invalid_request" => "invalid_request",
        "window_unavailable" => "window_unavailable",
        "input_unsupported" => "input_unsupported",
        "input_unavailable" => "input_unavailable",
        "input_focus_required" => "input_focus_required",
        "desktop_monitor_unavailable" => "desktop_monitor_unavailable",
        "desktop_capture_unavailable" => "desktop_capture_unavailable",
        "desktop_geometry_fixed" => "desktop_geometry_fixed",
        "desktop_close_unavailable" => "desktop_close_unavailable",
        _ => "capture_failed",
    }
}
struct Reader(tokio::task::JoinHandle<()>);
impl Drop for Reader {
    fn drop(&mut self) {
        self.0.abort();
    }
}
pub async fn client(
    socket: LocalStream,
    state: Arc<Mutex<State>>,
    local: bool,
    system: Box<dyn WindowSystem>,
) -> Result<()> {
    let (read, mut write) = socket.into_split();
    let mut read = BufReader::new(read);
    let hello = tokio::time::timeout(
        Duration::from_secs(10),
        framing::line_limit(&mut read, 64 * 1024),
    )
    .await??;
    ensure!(
        hello == json!({"type":"hello","version":1}),
        "invalid hello"
    );
    let mut client = Client {
        id: uuid::Uuid::new_v4(),
        state,
        system,
        session: None,
        video: false,
        color: Default::default(),
        fps: 30,
        flow: video_flow::VideoFlow::default(),
        sequence: 0,
        geometry: None,
        revision: 0,
        video_epoch: 0,
    };
    client.send(&mut write,json!({"type":"hello","capabilities":["native.h264","video.color.444-atlas-v1","video.color.444-atlas-v2","video.keyframe","video.qos","video.feedback.timing.v1","platform.windows","desktop.list","desktop.start","node.info","input.key.logical","input.text.commit","clipboard.remote-only","window.list","session.list","session.resume"],"video_max_fps":paneferry_protocol::MAX_VIDEO_FPS})).await?;
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    // Dedicated reader preserves partially read JSON when a capture timer fires.
    let _reader = Reader(tokio::spawn(async move {
        loop {
            let result = framing::line_limit(&mut read, 64 * 1024).await;
            let failed = result.is_err();
            if tx
                .send(result.map(|value| (value, std::time::Instant::now())))
                .await
                .is_err()
                || failed
            {
                break;
            }
        }
    }));
    // Exact deadlines avoid quantizing 60 fps to 50 fps (and 30 to 25) on a
    // 10 ms polling clock. After each input burst, give capture/timers a chance
    // to run without making further input depend on a pending video frame.
    let mut next_frame = tokio::time::Instant::now();
    let mut input_burst = 0;
    let mut capture: Option<paneferry_backend_windows::FrameTask> = None;
    let mut capture_epoch = 0;
    loop {
        tokio::select! {
            biased;
            request=rx.recv(), if input_burst < 8 =>{
                input_burst += 1;
                let (value,received)=request.ok_or_else(||anyhow::anyhow!("connection closed"))??;
                if value["type"].as_str().is_some_and(|kind| kind.starts_with("input.") || kind=="window.focus") {
                    paneferry_backend_windows::metrics::sample("input.dispatch_delay",received.elapsed());
                }
                let result=match request::parse(value){Ok(request)=>client.handle(request,&mut write,local).await,Err(error)=>Err(error)};
                match result{Ok(true)=>return Ok(()),Ok(false)=>{},Err(error)=>{client.send(&mut write,json!({"type":"error","code":public_error(&error)})).await?;}}
            },
            result = async { capture.as_mut().unwrap().await }, if capture.is_some() => {
                capture = None;
                if capture_epoch == client.video_epoch {
                    match result {
                        Ok(frame) => client.finish_tick(&mut write, frame).await?,
                        Err(error) => {
                            eprintln!("PaneFerry Windows frame error: {error:#}");
                            let code=public_error(&error);
                            if code=="session_exited" {
                                if let Some((id,_))=&client.session { client.state.lock().unwrap().sessions.remove(id); }
                                client.send(&mut write,json!({"type":"session.exited"})).await?;
                            } else {
                                client.send(&mut write,json!({"type":"error","code":code})).await?;
                            }
                            return Err(error);
                        }
                    }
                }
            },
            _=tokio::time::sleep_until(next_frame), if capture.is_none()=>{
                input_burst = 0;
                let period = Duration::from_secs_f64(1.0/client.fps as f64);
                next_frame += period;
                let now = tokio::time::Instant::now();
                // Preserve cadence across sub-millisecond timer rounding, but
                // never run a burst of overdue captures after a long stall.
                if next_frame <= now { next_frame = now + period; }
                if client.state.lock().unwrap().stopping {return Ok(());}
                if client.session.is_none() {continue;}
                match client.begin_tick(&mut write).await {
                    Ok(task) => { capture = task; capture_epoch = client.video_epoch; },
                    Err(error) => {
                    let code=public_error(&error);
                    if code == "capture_failed" {
                        // Local diagnostics only; never include request payloads,
                        // window titles, captured pixels or authentication data.
                        eprintln!("PaneFerry Windows frame error: {error:#}");
                    }
                    if code=="session_exited" {
                        if let Some((id,_))=&client.session {client.state.lock().unwrap().sessions.remove(id);}
                        client.send(&mut write,json!({"type":"session.exited"})).await?;}
                    else{client.send(&mut write,json!({"type":"error","code":code})).await?;}
                    bail!("publisher stream ended");
                    }
                }
            },
            _=tokio::task::yield_now(), if input_burst >= 8 => {
                input_burst = 0;
            }
        }
    }
}

#[cfg(test)]
mod race_tests {
    use super::*;
    #[test]
    fn request_binding_is_rechecked_after_another_client_claims_it() {
        let state = State::new("test".into(), true);
        let mut state = state.lock().unwrap();
        assert!(prior_request(&state, "request", "window-b")
            .unwrap()
            .is_none());
        // Represents the other client's insertion during the geometry check.
        state
            .requests
            .insert("request".into(), ("window-a".into(), "session-a".into()));
        assert_eq!(
            prior_request(&state, "request", "window-b")
                .unwrap_err()
                .to_string(),
            "request_id_conflict"
        );
        assert_eq!(
            prior_request(&state, "request", "window-a").unwrap(),
            Some("session-a")
        );
    }

    #[test]
    fn attach_failure_cleanup_tolerates_retirement_and_never_clears_a_new_owner() {
        let state = State::new("test".into(), true);
        let mut state = state.lock().unwrap();
        let failed = uuid::Uuid::new_v4();
        // Another client retired the target while geometry/attach was running.
        release_controller(&mut state, "s", failed, true);
        let current = uuid::Uuid::new_v4();
        state.sessions.insert(
            "s".into(),
            Session {
                app_id: "window".into(),
                target: Target {
                    monitor: None,
                    id: "window".into(),
                    handle: 1,
                    pid: 1,
                    started: 1,
                    generation: 1,
                    name: "test".into(),
                    application_key: "exe-test".into(),
                    auto_connectable: true,
                },
                controller: Some(current),
            },
        );
        release_controller(&mut state, "s", failed, true);
        assert_eq!(state.sessions["s"].controller, Some(current));
        release_controller(&mut state, "s", current, true);
        assert!(!state.sessions.contains_key("s"));
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::*;
    use paneferry_backend_windows::{Input, VideoFrame};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "requires windows-composed-motion.ps1 and PANEFERRY_COMPOSED_MOTION_PID"]
    async fn moving_owned_window_preserves_encoded_reference_chain() -> Result<()> {
        use paneferry_backend_windows::{color::ColorMode, CaptureRuntime, PublishingDesktop};
        let pid: u32 = std::env::var("PANEFERRY_COMPOSED_MOTION_PID")?.parse()?;
        let _runtime = CaptureRuntime::new()?;
        let mut system = PublishingDesktop::new()?;
        let target = system
            .windows()?
            .into_iter()
            .find(|w| w.pid == pid && w.name == "PaneFerry composed motion verification")
            .ok_or_else(|| anyhow::anyhow!("composed motion fixture missing"))?;
        system.set_video_color(ColorMode::Full)?;
        system.attach(&target, 60)?;
        let geometry = system.geometry(&target)?;
        let mut client = Client {
            id: uuid::Uuid::new_v4(),
            state: State::new("test".into(), true),
            system: Box::new(system),
            session: Some(("fixture".into(), target)),
            video: true,
            color: ColorMode::Full,
            fps: 60,
            flow: video_flow::VideoFlow::default(),
            sequence: 0,
            geometry: Some(geometry),
            revision: 1,
            video_epoch: 0,
        };
        let mut streams = std::collections::HashSet::new();
        let mut layouts = std::collections::HashSet::new();
        let mut bitstream = Vec::new();
        let mut frames = 0;
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while frames < 120 && std::time::Instant::now() < deadline {
            let mut output = Vec::new();
            client.tick(&mut output).await?;
            layouts.insert(client.geometry.as_ref().unwrap().content_revision);
            for line in output
                .split(|b| *b == b'\n')
                .filter(|line| !line.is_empty())
            {
                let event: Value = serde_json::from_slice(line)?;
                if event["type"] != "frame" {
                    continue;
                }
                ensure!(
                    event["geometry_revision"] == 1,
                    "internal layout changed wire geometry"
                );
                streams.insert(event["stream_id"].as_u64().unwrap());
                if frames == 0 {
                    ensure!(event["key_frame"] == true, "missing initial IDR");
                }
                bitstream.extend(STANDARD.decode(event["data"].as_str().unwrap())?);
                frames += 1;
            }
            tokio::time::sleep(Duration::from_millis(16)).await;
        }
        ensure!(
            frames == 120 && layouts.len() >= 10,
            "fixture must animate multiple layouts"
        );
        ensure!(
            streams.len() == 1,
            "internal motion restarted the video stream"
        );
        if let Some(dir) = std::env::var_os("PANEFERRY_VIDEO_PROBE_DIR") {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(
                std::path::PathBuf::from(dir).join("composed-motion.h264"),
                bitstream,
            )?;
        }
        eprintln!(
            "COMPOSED_MOTION frames={frames} layouts={} streams={}",
            layouts.len(),
            streams.len()
        );
        Ok(())
    }

    struct ResizingDesktop {
        resized: bool,
        layout_only: bool,
        stale_frame: bool,
        keyframes: Arc<AtomicUsize>,
    }
    impl WindowSystem for ResizingDesktop {
        fn windows(&mut self) -> Result<Vec<Target>> {
            Ok(Vec::new())
        }
        fn geometry(&mut self, _: &Target) -> Result<Geometry> {
            Ok(Geometry {
                content_revision: if self.resized {
                    if self.stale_frame {
                        2
                    } else {
                        1
                    }
                } else {
                    0
                },
                title: "resizing".into(),
                width: if self.resized && !self.layout_only {
                    800
                } else {
                    640
                },
                height: 480,
                visible: true,
            })
        }
        fn attach(&mut self, _: &Target, _: u32) -> Result<()> {
            Ok(())
        }
        fn keyframe(&mut self, _: &Target) -> Result<()> {
            self.keyframes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn frame(&mut self, _: &Target) -> Result<Option<VideoFrame>> {
            self.resized = true;
            Ok(Some(VideoFrame {
                content_revision: 1,
                data: vec![0, 0, 0, 1, 0x65],
                width: if self.layout_only { 640 } else { 800 },
                height: 480,
                stream_id: 2,
                key_frame: !self.layout_only,
            }))
        }
        fn input(&mut self, _: &Target, _: Input) -> Result<()> {
            Ok(())
        }
        fn detach(&mut self) {}
    }
    #[tokio::test]
    async fn capture_layout_is_published_without_discarding_current_video() {
        verify_geometry_race(false, false).await;
        verify_geometry_race(true, false).await;
    }
    #[tokio::test]
    async fn obsolete_capture_still_requests_a_new_keyframe() {
        verify_geometry_race(false, true).await;
        verify_geometry_race(true, true).await;
    }
    async fn verify_geometry_race(layout_only: bool, stale_frame: bool) {
        let keyframes = Arc::new(AtomicUsize::new(0));
        let target = Target {
            monitor: None,
            id: "resize".into(),
            handle: 1,
            pid: 1,
            started: 1,
            generation: 1,
            name: "resizing".into(),
            application_key: "exe-test".into(),
            auto_connectable: true,
        };
        let mut system = ResizingDesktop {
            resized: false,
            layout_only,
            stale_frame,
            keyframes: keyframes.clone(),
        };
        let geometry = system.geometry(&target).unwrap();
        let mut client = Client {
            id: uuid::Uuid::new_v4(),
            state: State::new("test".into(), true),
            system: Box::new(system),
            session: Some(("session".into(), target)),
            video: true,
            color: Default::default(),
            fps: 30,
            flow: video_flow::VideoFlow::default(),
            sequence: 0,
            geometry: Some(geometry),
            revision: 1,
            video_epoch: 0,
        };
        let mut output = Vec::new();
        client.tick(&mut output).await.unwrap();
        if stale_frame {
            assert!(output.is_empty(), "obsolete capture must not be relabelled");
            assert_eq!(keyframes.load(Ordering::SeqCst), 1);
            return;
        }
        assert_eq!(keyframes.load(Ordering::SeqCst), 0);
        let events: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        if layout_only {
            assert_eq!(
                events.len(),
                1,
                "internal layout must not invalidate wire geometry"
            );
            assert_eq!(events[0]["type"], "frame");
            assert_eq!(events[0]["geometry_revision"], 1);
            assert_eq!(
                events[0]["key_frame"], false,
                "keep the coded reference chain"
            );
            assert_eq!(client.geometry.as_ref().unwrap().content_revision, 1);
            return;
        }
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["type"], "window.metadata");
        assert_eq!(events[0]["width"], if layout_only { 640 } else { 800 });
        assert_eq!(events[1]["type"], "frame");
        assert_eq!(
            events[1]["geometry_revision"],
            events[0]["geometry_revision"]
        );
        assert_eq!(events[1]["geometry_revision"], 2);
        assert_eq!(events[1]["key_frame"], true);
    }
}
