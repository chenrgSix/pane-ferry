mod request;
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
    fps: u32,
    sequence: u64,
    geometry: Option<Geometry>,
    revision: u64,
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
        json!({"window_id":1,"parent_id":null,"title":g.title,"role":"toplevel","x":0,"y":0,"width":g.width,"height":g.height,"visible":g.visible,"scale_factor":1,"geometry_revision":self.revision})
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
            let geometry = self.system.geometry(&target)?;
            self.system.attach(&target, self.fps)?;
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
                    json!({"type":"node.info","device_id":state.device_id,"name":"Windows","platform":"windows","publishing_supported":true,"receiving_supported":false,"sharing_enabled":state.sharing,"session_count":state.sessions.len()})
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
                self.send(write,json!({"type":"app.list","apps":[],"discovery_supported":true,"discovery_error":null})).await?;
            }
            Request::Desktops => {
                self.shared()?;
                self.send(write,json!({"type":"desktop.list","monitors":[],"unavailable_reason":"desktop_mode_unsupported"})).await?;
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
                    windows.push(json!({"id":target.id,"window_id":windows.len()+1,"name":target.name,"title":g.title,"icon_png":null,"minimized":false,"controlled":existing.is_some_and(|(_,s)|s.controller.is_some()),"session_id":existing.map(|(id,_)|id)}));
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
                        .map(|(id, s)| (id.clone(), s.target.clone(), s.controller.is_some()))
                        .collect()
                };
                let mut values = Vec::new();
                for (id, target, controlled) in sessions {
                    if let Ok(g) = self.system.geometry(&target) {
                        values.push(json!({"session_id":id,"app_id":target.id,"backend":"native","display":0,"controlled":controlled,"window_count":if g.visible{1}else{0}}));
                    }
                }
                self.send(write, json!({"type":"session.list","sessions":values}))
                    .await?;
            }
            Request::KeyFrame { window_id } => {
                self.shared()?;
                ensure!(window_id == 1, "window_unavailable");
                let (_, target) = self
                    .session
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("session_expired"))?;
                self.system.keyframe(target)?;
            }
            Request::Video {
                codec,
                max_fps,
                feedback,
            } => {
                ensure!(
                    self.session.is_none() && codec == "h264" && (1..=90).contains(&max_fps),
                    "invalid_request"
                );
                // No video.qos advertised yet; accept legacy negotiation but never claim feedback support.
                let _ = feedback;
                self.video = true;
                self.fps = max_fps.min(30);
                self.send(
                    write,
                    json!({"type":"video.ready","codec":"h264","max_fps":self.fps}),
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
                let target = {
                    let s = self.state.lock().unwrap();
                    if let Some(id) = prior_request(&s, &request_id, &app_id)? {
                        s.sessions
                            .get(id)
                            .ok_or_else(|| anyhow::anyhow!("session_exited"))?
                            .target
                            .clone()
                    } else {
                        s.approved
                            .get(&app_id)
                            .ok_or_else(|| anyhow::anyhow!("app_not_approved"))?
                            .clone()
                    }
                };
                self.system.geometry(&target)?; // Recheck process creation time and current user before acquiring.
                let id = {
                    let mut s = self.state.lock().unwrap();
                    // Another client can claim this request while geometry is
                    // checked outside the mutex. Revalidate its app binding.
                    if let Some(id) = prior_request(&s, &request_id, &app_id)? {
                        id.to_owned()
                    } else {
                        ensure!(s.requests.len() < 1024, "request_history_limit");
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
                                    target,
                                    controller: None,
                                },
                            );
                            id
                        };
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
                self.system.input(target, input)?;
            }
        }
        Ok(false)
    }
    async fn tick<W: AsyncWrite + Unpin>(&mut self, write: &mut W) -> Result<()> {
        self.shared()?;
        let target = self.session.as_ref().unwrap().1.clone();
        let geometry = self.system.geometry(&target)?;
        if self.geometry.as_ref() != Some(&geometry) {
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
        }
        if let Some(frame) = self.system.frame(&target)? {
            // Old queued frames must never be relabelled with the new geometry revision.
            let geometry = self.geometry.as_ref().unwrap();
            if frame.width != geometry.width || frame.height != geometry.height || !geometry.visible
            {
                // The source window may resize after the metadata snapshot but
                // before capture. Do not disconnect or label this frame with an
                // obsolete revision. Dropping an encoded access unit also drops
                // a decoder reference, so restart the stream before trying again.
                self.system.keyframe(&target)?;
                return Ok(());
            }
            ensure!(frame.data.len() <= 12 * 1024 * 1024, "capture_failed");
            self.send(write,json!({"type":"frame","window_id":1,"geometry_revision":self.revision,"codec":"h264","width":frame.width,"height":frame.height,"encoded_width":frame.width+frame.width%2,"encoded_height":frame.height+frame.height%2,"stream_id":frame.stream_id,"key_frame":frame.key_frame,"data":STANDARD.encode(frame.data)})).await?;
        }
        Ok(())
    }
}
fn public_error(error: &anyhow::Error) -> &'static str {
    match error.to_string().as_str() {
        "sharing_disabled" => "sharing_disabled",
        "desktop_locked" => "desktop_locked",
        "session_exited" => "session_exited",
        "session_expired" => "session_expired",
        "session_controlled" => "session_controlled",
        "session_limit" => "session_limit",
        "request_history_limit" => "request_history_limit",
        "request_id_conflict" => "request_id_conflict",
        "app_not_approved" => "app_not_approved",
        "invalid_request" => "invalid_request",
        "window_unavailable" => "window_unavailable",
        "input_unsupported" => "input_unsupported",
        "input_unavailable" => "input_unavailable",
        "input_focus_required" => "input_focus_required",
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
        fps: 30,
        sequence: 0,
        geometry: None,
        revision: 0,
    };
    client.send(&mut write,json!({"type":"hello","capabilities":["native.h264","video.keyframe","platform.windows","node.info","input.key.logical","input.text.commit","clipboard.remote-only","window.list","session.list","session.resume"],"video_max_fps":30})).await?;
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    // Dedicated reader preserves partially read JSON when a capture timer fires.
    let _reader = Reader(tokio::spawn(async move {
        loop {
            let result = framing::line_limit(&mut read, 64 * 1024).await;
            let failed = result.is_err();
            if tx.send(result).await.is_err() || failed {
                break;
            }
        }
    }));
    let mut tick = tokio::time::interval(Duration::from_millis(10));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_frame = std::time::Instant::now();
    loop {
        tokio::select! {
            request=rx.recv()=>{
                let value=request.ok_or_else(||anyhow::anyhow!("connection closed"))??;
                let result=match request::parse(value){Ok(request)=>client.handle(request,&mut write,local).await,Err(error)=>Err(error)};
                match result{Ok(true)=>return Ok(()),Ok(false)=>{},Err(error)=>{client.send(&mut write,json!({"type":"error","code":public_error(&error)})).await?;}}
            },
            _=tick.tick()=>{
                if client.state.lock().unwrap().stopping {return Ok(());}
                if client.session.is_none() {continue;}
                if last_frame.elapsed()<Duration::from_secs_f64(1.0/client.fps as f64){continue;}
                last_frame=std::time::Instant::now();
                if let Err(error)=client.tick(&mut write).await{
                    let code=public_error(&error);
                    if code=="session_exited" {
                        if let Some((id,_))=&client.session {client.state.lock().unwrap().sessions.remove(id);}
                        client.send(&mut write,json!({"type":"session.exited"})).await?;}
                    else{client.send(&mut write,json!({"type":"error","code":code})).await?;}
                    bail!("publisher stream ended");
                }
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
                target: Target {
                    id: "window".into(),
                    handle: 1,
                    pid: 1,
                    started: 1,
                    generation: 1,
                    name: "test".into(),
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

    struct ResizingDesktop {
        resized: bool,
        keyframes: Arc<AtomicUsize>,
    }
    impl WindowSystem for ResizingDesktop {
        fn windows(&mut self) -> Result<Vec<Target>> {
            Ok(Vec::new())
        }
        fn geometry(&mut self, _: &Target) -> Result<Geometry> {
            Ok(Geometry {
                title: "resizing".into(),
                width: if self.resized { 800 } else { 640 },
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
                data: vec![0, 0, 0, 1, 0x65],
                width: 800,
                height: 480,
                stream_id: 2,
                key_frame: true,
            }))
        }
        fn input(&mut self, _: &Target, _: Input) -> Result<()> {
            Ok(())
        }
        fn detach(&mut self) {}
    }
    #[tokio::test]
    async fn resize_between_metadata_and_capture_restarts_video_without_disconnect() {
        let keyframes = Arc::new(AtomicUsize::new(0));
        let target = Target {
            id: "resize".into(),
            handle: 1,
            pid: 1,
            started: 1,
            generation: 1,
            name: "resizing".into(),
        };
        let mut system = ResizingDesktop {
            resized: false,
            keyframes: keyframes.clone(),
        };
        let geometry = system.geometry(&target).unwrap();
        let mut client = Client {
            id: uuid::Uuid::new_v4(),
            state: State::new("test".into(), true),
            system: Box::new(system),
            session: Some(("session".into(), target)),
            video: true,
            fps: 30,
            sequence: 0,
            geometry: Some(geometry),
            revision: 1,
        };
        let mut output = Vec::new();
        client.tick(&mut output).await.unwrap();
        assert!(
            output.is_empty(),
            "raced frame must not carry stale metadata"
        );
        assert_eq!(keyframes.load(Ordering::SeqCst), 1);
        client.tick(&mut output).await.unwrap();
        let events: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["type"], "window.metadata");
        assert_eq!(events[0]["width"], 800);
        assert_eq!(events[1]["type"], "frame");
        assert_eq!(
            events[1]["geometry_revision"],
            events[0]["geometry_revision"]
        );
        assert_eq!(events[1]["geometry_revision"], 2);
        assert_eq!(events[1]["key_frame"], true);
    }
}
