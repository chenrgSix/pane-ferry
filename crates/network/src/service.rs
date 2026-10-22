use crate::{
    framing::{self, Packet},
    identity::{self, Identity, Peer},
};
use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use quinn::{Connection, RecvStream, SendStream};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    net::SocketAddr,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{mpsc, Semaphore},
    task::JoinSet,
};

const TIMEOUT: Duration = Duration::from_secs(15);
const PEER_LIMIT: usize = 32;
#[derive(Clone)]
pub struct Config {
    pub state: PathBuf,
    pub publisher: PathBuf,
    pub listen: SocketAddr,
    pub name: String,
    pub device_id: Option<String>,
    pub bulk_agent: Option<PathBuf>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invitation {
    version: u32,
    pub peer: Peer,
    token: String,
    expires: u64,
}
struct Pending {
    token: String,
    expires: u64,
}
struct State {
    peers: HashMap<String, Peer>,
    pending: Option<Pending>,
    connections: HashMap<String, Vec<Connection>>,
    dialers: HashMap<String, Arc<tokio::sync::Mutex<()>>>,
    media: HashMap<(usize, String), mpsc::Sender<Packet>>,
}
pub struct Service {
    config: Config,
    identity: Identity,
    endpoint: quinn::Endpoint,
    state: Mutex<State>,
    slots: Arc<Semaphore>,
    _lock: fs::File,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn route_path(state: &Path, id: &str) -> PathBuf {
    state.join(format!("r-{id}.sock"))
}
fn bind(path: &Path) -> Result<UnixListener> {
    if path.symlink_metadata().is_ok() {
        ensure!(
            path.symlink_metadata()?.file_type().is_socket(),
            "invalid socket"
        );
        fs::remove_file(path)?;
    }
    let socket = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(socket)
}
use std::os::unix::fs::FileTypeExt;
impl Service {
    pub fn new(config: Config) -> Result<Arc<Self>> {
        remoteapp_core::node::private_directory(&config.state)?;
        let lock_path = config.state.join("service.lock");
        ensure!(!lock_path.is_symlink(), "invalid service lock");
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(lock_path)?;
        lock.try_lock().context("network service already running")?;
        let identity = Identity::load(&config.state, config.device_id.as_deref())?;
        let peers: HashMap<String, Peer> = if config.state.join("peers.json").exists() {
            identity::read(&config.state.join("peers.json"))?
        } else {
            HashMap::new()
        };
        ensure!(peers.len() <= PEER_LIMIT, "too many peers");
        for (id, peer) in &peers {
            peer.validate()?;
            ensure!(id == &peer.device_id && id != &identity.id, "invalid peer");
        }
        let endpoint = quinn::Endpoint::server(identity.server()?, config.listen)?;
        Ok(Arc::new(Self {
            config,
            identity,
            endpoint,
            state: Mutex::new(State {
                peers,
                pending: None,
                connections: HashMap::new(),
                dialers: HashMap::new(),
                media: HashMap::new(),
            }),
            slots: Arc::new(Semaphore::new(64)),
            _lock: lock,
        }))
    }
    pub fn address(&self) -> SocketAddr {
        self.endpoint.local_addr().unwrap()
    }
    pub fn id(&self) -> &str {
        &self.identity.id
    }
    fn local_peer(&self) -> Peer {
        Peer {
            device_id: self.identity.id.clone(),
            name: self.config.name.clone(),
            certificate: self.identity.certificate.clone(),
            address: self.address().to_string(),
        }
    }
    pub fn invite(&self) -> Result<String> {
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let expires = now() + 300;
        self.state.lock().unwrap().pending = Some(Pending {
            token: token.clone(),
            expires,
        });
        Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Invitation {
            version: 2,
            peer: self.local_peer(),
            token,
            expires,
        })?))
    }
    fn trusted(&self, peer: &str, certificate: &str) -> bool {
        self.state
            .lock()
            .unwrap()
            .peers
            .get(peer)
            .is_some_and(|p| p.certificate == certificate)
    }
    fn persist_peer(&self, peer: Peer) -> Result<()> {
        peer.validate()?;
        ensure!(peer.device_id != self.identity.id, "cannot pair with self");
        let mut state = self.state.lock().unwrap();
        if let Some(old) = state.peers.get(&peer.device_id) {
            ensure!(
                old.certificate == peer.certificate,
                "device identity changed; revoke before pairing again"
            );
        }
        ensure!(
            state.peers.contains_key(&peer.device_id) || state.peers.len() < PEER_LIMIT,
            "too many peers"
        );
        let mut next = state.peers.clone();
        next.insert(peer.device_id.clone(), peer);
        identity::save(&self.config.state.join("peers.json"), &next)?;
        state.peers = next;
        Ok(())
    }
    pub async fn pair(self: &Arc<Self>, invitation: &str, address: &str) -> Result<Peer> {
        ensure!(invitation.len() < 16384, "invalid invitation");
        let invite: Invitation = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(invitation)?)?;
        ensure!(
            invite.version == 2 && invite.expires >= now() && invite.expires <= now() + 600,
            "invitation expired"
        );
        let mut peer = invite.peer;
        peer.address = address.parse::<SocketAddr>()?.to_string();
        peer.validate()?;
        // Reject a changed pin before sending the invitation secret.
        if let Some(old) = self.state.lock().unwrap().peers.get(&peer.device_id) {
            ensure!(
                old.certificate == peer.certificate,
                "device identity changed"
            );
        }
        let connection = tokio::time::timeout(
            TIMEOUT,
            self.endpoint.connect_with(
                self.identity.client(&peer.certificate)?,
                address.parse()?,
                "lan-remoteapp",
            )?,
        )
        .await??;
        ensure!(
            identity::remote_certificate(&connection)? == peer.certificate,
            "certificate mismatch"
        );
        let (mut send, mut recv) = connection.open_bi().await?;
        Packet::new(
            json!({"op":"pair", "token":invite.token,"peer":self.local_peer()}),
            0,
        )?
        .write(&mut send)
        .await?;
        let answer = tokio::time::timeout(TIMEOUT, Packet::read(&mut recv)).await??;
        ensure!(answer.value["ok"] == true, "pairing rejected");
        send.finish()?;
        self.persist_peer(peer.clone())?;
        self.register(&peer.device_id, &connection)?;
        let service = self.clone();
        let id = peer.device_id.clone();
        tokio::spawn(async move {
            service.connected(connection, id).await;
        });
        Ok(peer)
    }
    fn register(&self, id: &str, connection: &Connection) -> Result<()> {
        ensure!(
            self.trusted(id, &identity::remote_certificate(connection)?),
            "untrusted device"
        );
        let mut state = self.state.lock().unwrap();
        let list = state.connections.entry(id.into()).or_default();
        list.retain(|c| c.close_reason().is_none());
        ensure!(list.len() < 4, "device connection limit");
        list.push(connection.clone());
        Ok(())
    }
    pub fn revoke(&self, id: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let mut peers = state.peers.clone();
        peers.remove(id);
        identity::save(&self.config.state.join("peers.json"), &peers)?;
        state.peers = peers;
        if let Some(list) = state.connections.remove(id) {
            for c in list {
                c.close(1u32.into(), b"device revoked");
            }
        }
        Ok(())
    }
    pub fn status(&self) -> Value {
        let state = self.state.lock().unwrap();
        let mut peers: Vec<_> = state.peers.values().map(|p| json!({"device_id":p.device_id,"name":p.name,"address":p.address,"fingerprint":identity::fingerprint(&p.certificate).unwrap_or_default(),"socket":route_path(&self.config.state,&p.device_id),"transport":"quic","generation":p.device_id,"connected":state.connections.get(&p.device_id).is_some_and(|list| list.iter().any(|c| c.close_reason().is_none()))})).collect();
        peers.sort_by(|a, b| a["device_id"].as_str().cmp(&b["device_id"].as_str()));
        json!({"version":2,"device_id":self.id(),"name":self.config.name,"listen":self.address(),"fingerprint":identity::fingerprint(&self.identity.certificate).unwrap_or_default(),"peers":peers})
    }
    pub async fn run(self: Arc<Self>) -> Result<()> {
        let listener = bind(&self.config.state.join("control.sock"))?;
        let service = self.clone();
        let routes = tokio::spawn(async move { service.routes().await });
        let service = self.clone();
        let incoming = tokio::spawn(async move {
            while let Some(incoming) = service.endpoint.accept().await {
                let Ok(permit) = service.slots.clone().try_acquire_owned() else {
                    incoming.refuse();
                    continue;
                };
                let service = service.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Ok(Ok(connection)) = tokio::time::timeout(TIMEOUT, incoming).await {
                        service.accept(connection).await;
                    }
                });
            }
        });
        loop {
            let (socket, _) = listener.accept().await?;
            if !same_user(&socket) {
                continue;
            }
            let Ok(permit) = self.slots.clone().try_acquire_owned() else {
                continue;
            };
            let service = self.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let _ = service.management(socket).await;
            });
            if routes.is_finished() || incoming.is_finished() {
                bail!("network listener stopped");
            }
        }
    }
    async fn accept(self: Arc<Self>, connection: Connection) {
        let result = tokio::time::timeout(TIMEOUT, async {
            let (mut send, mut recv) = connection.accept_bi().await?;
            let request = Packet::read(&mut recv).await?.value;
            let cert = identity::remote_certificate(&connection)?;
            let id = match request["op"].as_str() {
                Some("auth") => {
                    let id = request["device_id"].as_str().context("missing device")?;
                    ensure!(self.trusted(id, &cert), "untrusted device");
                    id.to_owned()
                }
                Some("pair") => {
                    let mut peer: Peer = serde_json::from_value(request["peer"].clone())?;
                    ensure!(peer.certificate == cert, "certificate mismatch");
                    peer.validate()?;
                    // Use the authenticated connection's observed address for reverse dialing.
                    peer.address = connection.remote_address().to_string();
                    {
                        let mut state = self.state.lock().unwrap();
                        let pending = state.pending.as_ref().context("pairing disabled")?;
                        ensure!(
                            pending.expires >= now()
                                && request["token"].as_str() == Some(&pending.token),
                            "invalid invitation"
                        );
                        state.pending = None;
                    }
                    let id = peer.device_id.clone();
                    self.persist_peer(peer)?;
                    id
                }
                _ => bail!("unauthorized operation"),
            };
            self.register(&id, &connection)?;
            Packet::new(json!({"ok":true}), 0)?.write(&mut send).await?;
            send.finish()?;
            Ok::<_, anyhow::Error>(id)
        })
        .await
        .map_err(anyhow::Error::from)
        .and_then(|r| r);
        match result {
            Ok(id) => self.connected(connection, id).await,
            Err(_) => connection.close(1u32.into(), b"authentication rejected"),
        }
    }
    async fn connection(self: &Arc<Self>, id: &str) -> Result<Connection> {
        let dialer = {
            let mut state = self.state.lock().unwrap();
            ensure!(state.peers.contains_key(id), "unpaired device");
            state
                .dialers
                .entry(id.to_owned())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _dial = dialer.lock().await;
        let peer = {
            let state = self.state.lock().unwrap();
            let peer = state.peers.get(id).context("unpaired device")?.clone();
            if let Some(connection) = state
                .connections
                .get(id)
                .and_then(|list| list.iter().rev().find(|c| c.close_reason().is_none()))
            {
                return Ok(connection.clone());
            }
            peer
        };
        let connection = tokio::time::timeout(
            TIMEOUT,
            self.endpoint.connect_with(
                self.identity.client(&peer.certificate)?,
                peer.address.parse()?,
                "lan-remoteapp",
            )?,
        )
        .await??;
        ensure!(
            identity::remote_certificate(&connection)? == peer.certificate,
            "certificate mismatch"
        );
        let (mut send, mut recv) = connection.open_bi().await?;
        Packet::new(json!({"op":"auth","device_id":self.id()}), 0)?
            .write(&mut send)
            .await?;
        ensure!(
            tokio::time::timeout(TIMEOUT, Packet::read(&mut recv))
                .await??
                .value["ok"]
                == true,
            "authentication rejected"
        );
        send.finish()?;
        self.register(id, &connection)?;
        let service = self.clone();
        let remote = id.to_owned();
        let clone = connection.clone();
        tokio::spawn(async move {
            service.connected(clone, remote).await;
        });
        Ok(connection)
    }
    async fn connected(self: Arc<Self>, connection: Connection, id: String) {
        let mut tasks = JoinSet::new();
        loop {
            tokio::select! {
                result = connection.accept_bi() => {
                    let Ok((send,recv)) = result else { break; };
                    if tasks.len() >= 128 || !self.trusted(&id,&identity::remote_certificate(&connection).unwrap_or_default()) { connection.close(1u32.into(),b"device limit or revoked"); break; }
                    let service = self.clone(); let conn = connection.clone();
                    tasks.spawn(async move { let _ = service.stream(conn,send,recv).await; });
                }
                _ = tasks.join_next(), if !tasks.is_empty() => {}
            }
        }
        tasks.abort_all();
        self.state
            .lock()
            .unwrap()
            .media
            .retain(|(cid, _), _| *cid != connection.stable_id());
    }
    async fn stream(
        self: Arc<Self>,
        connection: Connection,
        mut send: SendStream,
        mut recv: RecvStream,
    ) -> Result<()> {
        let head = tokio::time::timeout(TIMEOUT, Packet::read(&mut recv))
            .await??
            .value;
        match head["op"].as_str() {
            Some("rwp") => {
                let channel = head["channel"].as_str().context("missing channel")?;
                uuid::Uuid::parse_str(channel)?;
                let socket = UnixStream::connect(&self.config.publisher).await?;
                self.publish(connection, send, recv, socket, channel.to_owned())
                    .await
            }
            Some("media") => {
                let channel = head["channel"].as_str().context("missing channel")?;
                let sender = self
                    .state
                    .lock()
                    .unwrap()
                    .media
                    .get(&(connection.stable_id(), channel.to_owned()))
                    .context("unknown channel")?
                    .clone();
                send.finish()?;
                let result = async {
                    loop {
                        let packet = Packet::read(&mut recv).await?;
                        ensure!(
                            packet.value["type"] == "frame"
                                && packet.value["window_id"] == head["window_id"],
                            "invalid media"
                        );
                        tokio::time::timeout(TIMEOUT, sender.send(packet)).await??;
                    }
                    #[allow(unreachable_code)]
                    Ok::<(), anyhow::Error>(())
                }
                .await;
                let _ = sender
                    .send(Packet::new(json!({"type":"media.failed"}), 0)?)
                    .await;
                result
            }
            Some("bulk-upload" | "bulk-download") => self.bulk_server(send, recv, &head).await,
            _ => bail!("unknown operation"),
        }
    }
    async fn routes(self: Arc<Self>) {
        let mut listeners: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
        loop {
            let ids: Vec<_> = self.state.lock().unwrap().peers.keys().cloned().collect();
            listeners.retain(|id, task| {
                if ids.contains(id) && !task.is_finished() {
                    true
                } else {
                    task.abort();
                    let _ = fs::remove_file(route_path(&self.config.state, id));
                    false
                }
            });
            for id in ids {
                if listeners.contains_key(&id) {
                    continue;
                }
                let Ok(listener) = bind(&route_path(&self.config.state, &id)) else {
                    continue;
                };
                let service = self.clone();
                let peer = id.clone();
                listeners.insert(
                    id,
                    tokio::spawn(async move {
                        while let Ok((socket, _)) = listener.accept().await {
                            if !same_user(&socket) {
                                continue;
                            }
                            let service = service.clone();
                            let peer = peer.clone();
                            let Ok(permit) = service.slots.clone().try_acquire_owned() else {
                                continue;
                            };
                            tokio::spawn(async move {
                                let _permit = permit;
                                let _ = service.receive(&peer, socket).await;
                            });
                        }
                    }),
                );
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    async fn management(self: Arc<Self>, socket: UnixStream) -> Result<()> {
        let (read, mut write) = socket.into_split();
        let mut read = BufReader::new(read);
        let request = tokio::time::timeout(TIMEOUT, framing::line(&mut read)).await??;
        let result: Result<Value> = async {
            match request["op"].as_str() {
                Some("status") => Ok(self.status()),
                Some("invite") => Ok(json!({"invitation":self.invite()?,"expires_in":300})),
                Some("pair") => {
                    let peer = self
                        .pair(
                            request["invitation"]
                                .as_str()
                                .context("invitation required")?,
                            request["address"].as_str().context("address required")?,
                        )
                        .await?;
                    Ok(json!({"device_id":peer.device_id}))
                }
                Some("revoke") => {
                    self.revoke(request["device_id"].as_str().context("device required")?)?;
                    Ok(json!({"ok":true}))
                }
                Some("address") => {
                    let id = request["device_id"].as_str().context("device required")?;
                    let mut peer = self
                        .state
                        .lock()
                        .unwrap()
                        .peers
                        .get(id)
                        .context("unknown device")?
                        .clone();
                    peer.address = request["address"]
                        .as_str()
                        .context("address required")?
                        .parse::<SocketAddr>()?
                        .to_string();
                    self.persist_peer(peer)?;
                    Ok(json!({"ok":true}))
                }
                Some("bulk-upload" | "bulk-download") => {
                    self.bulk_client(&request, &mut read, &mut write).await?;
                    return Ok(Value::Null);
                }
                _ => bail!("unknown management operation"),
            }
        }
        .await;
        if request["op"] == "bulk-upload" || request["op"] == "bulk-download" {
            return result.map(|_| ());
        }
        framing::json(
            &mut write,
            &result.unwrap_or_else(|_| json!({"error":"network_operation_failed"})),
        )
        .await
    }
}
fn same_user(socket: &UnixStream) -> bool {
    socket
        .peer_cred()
        .is_ok_and(|cred| cred.uid() == unsafe { libc::geteuid() })
}
impl Service {
    async fn publish(
        self: &Arc<Self>,
        connection: Connection,
        mut send: SendStream,
        mut recv: RecvStream,
        socket: UnixStream,
        channel: String,
    ) -> Result<()> {
        send.set_priority(10)?;
        let (read, mut write) = socket.into_split();
        let mut read = BufReader::new(read);
        let mut media: HashMap<u64, mpsc::Sender<Packet>> = HashMap::new();
        let mut workers = JoinSet::new();
        let mut barrier = 0u64;
        let input = async {
            loop {
                let packet = Packet::read(&mut recv).await?;
                ensure!(
                    packet.payload.is_empty() && allowed_request(&packet.value),
                    "request not allowed"
                );
                framing::json(&mut write, &packet.value).await?;
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        };
        let output = async {
            loop {
                tokio::select! {
                    value = framing::line(&mut read) => {
                        let value = value?;
                        if value["type"] == "frame" {
                            let window = value["window_id"].as_u64().context("missing window")?;
                            if !media.contains_key(&window) {
                                ensure!(media.len() < 64,"too many windows");
                                let (tx,mut rx) = mpsc::channel::<Packet>(2); media.insert(window,tx);
                                let connection = connection.clone(); let channel = channel.clone();
                                workers.spawn(async move {
                                    let (mut send,_recv) = connection.open_bi().await?; send.set_priority(0)?;
                                    Packet::new(json!({"op":"media","channel":channel,"window_id":window}),0)?.write(&mut send).await?;
                                    while let Some(packet) = rx.recv().await { tokio::time::timeout(TIMEOUT,packet.write(&mut send)).await??; }
                                    send.finish()?; Ok::<_,anyhow::Error>(())
                                });
                            }
                            let mut packet = Packet::new(value,barrier)?; packet.reserve().await?;
                            // Encoded frames are never dropped. Overload closes only this
                            // session, whose existing reconnect obtains a fresh key frame.
                            media[&window].try_send(packet).map_err(|_|anyhow!("media queue full"))?;
                        } else {
                            barrier = barrier.checked_add(1).context("sequence overflow")?;
                            tokio::time::timeout(TIMEOUT,Packet::new(value,barrier)?.write(&mut send)).await??;
                        }
                    }
                    worker = workers.join_next(), if !workers.is_empty() => { worker.context("media worker ended")???; bail!("media stream ended"); }
                }
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        };
        let result = tokio::select! { r = input => r, r = output => r };
        workers.abort_all();
        result
    }
    pub async fn receive(self: &Arc<Self>, peer: &str, socket: UnixStream) -> Result<()> {
        let connection = self.connection(peer).await?;
        let channel = uuid::Uuid::new_v4().to_string();
        let (tx, mut rx) = mpsc::channel::<Packet>(8);
        let (control_tx, mut control_rx) = mpsc::channel::<Packet>(8);
        let key = (connection.stable_id(), channel.clone());
        self.state.lock().unwrap().media.insert(key.clone(), tx);
        let result = async {
            let (mut send,mut recv) = connection.open_bi().await?; send.set_priority(10)?;
            Packet::new(json!({"op":"rwp","channel":channel}),0)?.write(&mut send).await?;
            let (read,mut write) = socket.into_split(); let mut read = BufReader::new(read);
            let input = async {
                loop { let value = framing::line(&mut read).await?; Packet::new(value,0)?.write(&mut send).await?; }
                #[allow(unreachable_code)] Ok::<(),anyhow::Error>(())
            };
            let control = async {
                loop { control_tx.send(Packet::read(&mut recv).await?).await?; }
                #[allow(unreachable_code)] Ok::<(),anyhow::Error>(())
            };
            let output = async {
                let mut merge = Merge::default(); let mut pending: Vec<Packet> = Vec::new();
                loop {
                    tokio::select! {
                        packet = control_rx.recv() => {
                            let packet = packet.context("control ended")?; ensure!(packet.value["type"] != "frame" && packet.barrier == merge.barrier+1, "control ordering");
                            merge.barrier = packet.barrier; merge.observe(&packet.value)?;
                            let value = merge.deliver(packet); framing::json(&mut write,&value).await?;
                        }
                        packet = rx.recv(), if pending.len() < 8 => {
                            let packet = packet.context("media ended")?;
                            ensure!(packet.value["type"] == "frame", "media failed"); pending.push(packet);
                        }
                    }
                    let mut i = 0;
                    while i < pending.len() {
                        if pending[i].barrier <= merge.barrier {
                            let packet = pending.remove(i);
                            if merge.current(&packet) { let value = merge.deliver(packet); framing::json(&mut write,&value).await?; }
                        } else { i += 1; }
                    }
                }
                #[allow(unreachable_code)] Ok::<(),anyhow::Error>(())
            };
            tokio::select! { r = input => r, r = output => r, r = control => r }
        }.await;
        self.state.lock().unwrap().media.remove(&key);
        result
    }
    async fn bulk_server(
        &self,
        mut send: SendStream,
        mut recv: RecvStream,
        head: &Value,
    ) -> Result<()> {
        let agent = self
            .config
            .bulk_agent
            .as_ref()
            .context("bulk unsupported")?;
        // Bulk is subject to the same publisher sharing switch.
        let socket = UnixStream::connect(&self.config.publisher).await?;
        let (read, mut write) = socket.into_split();
        let mut read = BufReader::new(read);
        framing::json(&mut write, &json!({"version":1,"type":"hello"})).await?;
        framing::line(&mut read).await?;
        framing::json(&mut write, &json!({"version":1,"type":"node.info"})).await?;
        ensure!(
            framing::line(&mut read).await?["sharing_enabled"] == true,
            "sharing disabled"
        );
        let mut command = tokio::process::Command::new(agent);
        if head["op"] == "bulk-upload" {
            command.arg("clipboard-upload");
        } else {
            let id = head["transfer_id"].as_str().context("missing transfer")?;
            ensure!(
                !id.is_empty()
                    && id.len() <= 100
                    && id
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
                "invalid transfer"
            );
            command.args(["clipboard-download", id]);
        }
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut input = child.stdin.take().unwrap();
        let mut output = child.stdout.take().unwrap();
        send.set_priority(-10)?;
        tokio::time::timeout(Duration::from_secs(300), async {
            let upload = async {
                tokio::io::copy(&mut recv, &mut input).await?;
                input.shutdown().await?;
                drop(input);
                Ok::<_, anyhow::Error>(())
            };
            let download = async {
                let mut bytes = vec![0; 64 * 1024];
                loop {
                    let n = output.read(&mut bytes).await?;
                    if n == 0 {
                        break;
                    }
                    send.write_u32(n as u32).await?;
                    send.write_all(&bytes[..n]).await?;
                }
                Ok::<_, anyhow::Error>(())
            };
            tokio::try_join!(upload, download)?;
            ensure!(child.wait().await?.success(), "bulk failed");
            send.write_u32(0).await?;
            send.finish()?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(())
    }
    async fn bulk_client<R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin>(
        self: &Arc<Self>,
        request: &Value,
        read: &mut R,
        write: &mut W,
    ) -> Result<()> {
        let connection = self
            .connection(request["device_id"].as_str().context("device required")?)
            .await?;
        let (mut send, mut recv) = connection.open_bi().await?;
        send.set_priority(-10)?;
        Packet::new(request.clone(), 0)?.write(&mut send).await?;
        tokio::time::timeout(Duration::from_secs(300), async {
            let input = async {
                tokio::io::copy(read, &mut send).await?;
                send.finish()?;
                Ok::<_, anyhow::Error>(())
            };
            let output = async {
                receive_bulk(&mut recv, write, true).await?;
                write.shutdown().await?;
                Ok::<_, anyhow::Error>(())
            };
            tokio::try_join!(input, output)?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(())
    }
}
/// The compatibility edge assigns delivery sequence numbers after stream merge.
/// Frames retain their source control watermark and cannot precede metadata or
/// revive a destroyed/hidden/resized window. Each media stream preserves codec order.
#[derive(Default)]
struct Merge {
    barrier: u64,
    sequence: u64,
    reset: u64,
    windows: HashMap<u64, (u64, u64, bool)>,
}
impl Merge {
    fn observe(&mut self, value: &Value) -> Result<()> {
        match value["type"].as_str() {
            Some("session.snapshot" | "session.exited") => {
                self.windows.clear();
                self.reset = self.barrier;
                if let Some(windows) = value["windows"].as_array() {
                    for window in windows {
                        let mut window = window.clone();
                        window["type"] = json!("window.created");
                        self.observe(&window)?;
                    }
                }
            }
            Some("window.created" | "window.metadata" | "window.destroyed") => {
                if let Some(id) = value["window_id"].as_u64() {
                    ensure!(
                        self.windows.contains_key(&id) || self.windows.len() < 128,
                        "window history limit"
                    );
                    let previous =
                        self.windows
                            .get(&id)
                            .copied()
                            .unwrap_or((self.barrier, 0, true));
                    let destroyed =
                        value["type"] == "window.destroyed" || value["visible"] == false;
                    let revision = value["geometry_revision"].as_u64().unwrap_or(previous.1);
                    let generation = if value["type"] == "window.created" || destroyed || previous.2
                    {
                        self.barrier
                    } else {
                        previous.0
                    };
                    self.windows.insert(id, (generation, revision, destroyed));
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn current(&self, packet: &Packet) -> bool {
        packet.barrier >= self.reset
            && packet.value["window_id"]
                .as_u64()
                .and_then(|id| self.windows.get(&id))
                .is_some_and(|(born, revision, hidden)| {
                    !hidden
                        && packet.barrier >= *born
                        && packet.value["geometry_revision"].as_u64() == Some(*revision)
                })
    }
    fn deliver(&mut self, packet: Packet) -> Value {
        let mut value = packet.restore();
        if value.get("seq").is_some() {
            self.sequence += 1;
            value["seq"] = json!(self.sequence);
        }
        value
    }
}
fn allowed_request(value: &Value) -> bool {
    value["version"] == 1
        && matches!(
            value["type"].as_str(),
            Some(
                "hello"
                    | "node.info"
                    | "app.list"
                    | "window.list"
                    | "session.list"
                    | "video.enable"
                    | "app.launch"
                    | "session.resume"
                    | "session.detach"
                    | "window.focus"
                    | "window.close"
                    | "window.close.request"
                    | "window.configure"
                    | "input.pointer"
                    | "input.key"
                    | "input.key.logical"
                    | "input.text.commit"
                    | "clipboard.paste"
                    | "clipboard.copy"
                    | "clipboard.cut"
                    | "clipboard.paste.bundle"
                    | "clipboard.copy.bundle"
                    | "clipboard.inline.paste"
                    | "clipboard.inline.copy"
            )
        )
}

/// A success terminator prevents truncated/failed bulk commands from looking
/// like successful EOF to the existing clipboard implementation.
pub async fn receive_bulk<R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin>(
    read: &mut R,
    write: &mut W,
    framed: bool,
) -> Result<()> {
    loop {
        let size = read.read_u32().await?;
        ensure!(size <= 64 * 1024, "bulk chunk too large");
        if framed {
            write.write_u32(size).await?;
        }
        if size == 0 {
            write.flush().await?;
            return Ok(());
        }
        let mut bytes = vec![0; size as usize];
        read.read_exact(&mut bytes).await?;
        write.write_all(&bytes).await?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_frames_cannot_revive_windows_or_cross_geometry_changes() {
        let mut merge = Merge::default();
        merge.barrier = 1;
        merge.observe(&json!({"type":"session.snapshot","windows":[{"type":"window.created","window_id":7,"geometry_revision":4}]})).unwrap();
        let frame = |barrier, revision| {
            Packet::new(json!({"type":"frame","window_id":7,"geometry_revision":revision,"codec":"h264","data":"YQ=="}),barrier).unwrap()
        };
        assert!(merge.current(&frame(1, 4)));
        merge.barrier = 2;
        merge
            .observe(&json!({"type":"window.metadata","window_id":7,"geometry_revision":5}))
            .unwrap();
        assert!(!merge.current(&frame(1, 4)));
        assert!(merge.current(&frame(2, 5)));
        merge.barrier = 3;
        merge
            .observe(&json!({"type":"window.destroyed","window_id":7}))
            .unwrap();
        assert!(!merge.current(&frame(2, 5)));
        merge.barrier = 4;
        merge
            .observe(&json!({"type":"window.created","window_id":7,"geometry_revision":5}))
            .unwrap();
        assert!(!merge.current(&frame(2, 5)));
        assert!(merge.current(&frame(4, 5)));
    }
    #[tokio::test]
    async fn truncated_bulk_has_no_success_terminator() {
        let mut bytes: &[u8] = &[0, 0, 0, 3, b'a', b'b', b'c'];
        let mut output = Vec::new();
        assert!(receive_bulk(&mut bytes, &mut output, false).await.is_err());
        let mut complete: &[u8] = &[0, 0, 0, 3, b'a', b'b', b'c', 0, 0, 0, 0];
        output.clear();
        receive_bulk(&mut complete, &mut output, false)
            .await
            .unwrap();
        assert_eq!(output, b"abc");
    }
}
