use crate::{
    diagnostics::Diagnostics,
    framing::{self, Packet},
    identity::{self, Identity, Peer},
    media_queue::MediaQueue,
};
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use paneferry_platform::ipc::{bind, same_user, LocalStream as UnixStream};
use quinn::{Connection, RecvStream, SendStream};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, Semaphore},
    task::JoinSet,
    time::Instant,
};

const TIMEOUT: Duration = Duration::from_secs(15);
const PEER_LIMIT: usize = 32;

async fn within<T, E: Into<anyhow::Error>>(
    deadline: Instant,
    stage: &'static str,
    future: impl std::future::Future<Output = std::result::Result<T, E>>,
) -> Result<T> {
    tokio::time::timeout_at(deadline, future)
        .await
        .with_context(|| format!("{stage} timed out"))?
        .map_err(Into::into)
        .with_context(|| stage)
}

// A cancelled/failed authentication must not leave an unregistered connection
// alive. Registered connections are shared by independent window streams.
struct PendingConnection {
    connection: Connection,
    registered: bool,
}
impl Drop for PendingConnection {
    fn drop(&mut self) {
        if !self.registered {
            self.connection
                .close(1u32.into(), b"connection setup cancelled or failed");
        }
    }
}
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
#[derive(Clone, Serialize, Deserialize)]
struct Pending {
    claimed: Option<(String, String)>,
    token: String,
    expires: u64,
}
struct DeviceQuota {
    media: Arc<Semaphore>,
    control: Arc<Semaphore>,
    bulk: Arc<Semaphore>,
}
impl Default for DeviceQuota {
    fn default() -> Self {
        Self {
            media: Arc::new(Semaphore::new(64 * 1024 * 1024)),
            control: Arc::new(Semaphore::new(16 * 1024 * 1024)),
            bulk: Arc::new(Semaphore::new(2)),
        }
    }
}
struct State {
    peers: HashMap<String, Peer>,
    pending: Option<Pending>,
    quotas: HashMap<String, Arc<DeviceQuota>>,
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
    handshake_slots: Arc<Semaphore>,
    management_slots: Arc<Semaphore>,
    bulk_slots: Arc<Semaphore>,
    diagnostics: Diagnostics,
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
impl Service {
    pub fn new(config: Config) -> Result<Arc<Self>> {
        paneferry_core::node::private_directory(&config.state)?;
        let lock_path = config.state.join("service.lock");
        ensure!(!lock_path.is_symlink(), "invalid service lock");
        let lock = paneferry_platform::storage::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
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
        let pending: Option<Pending> = if config.state.join("invitation.json").exists() {
            identity::read::<Option<Pending>>(&config.state.join("invitation.json"))?
                .filter(|p| p.expires >= now() && p.expires <= now() + 600)
        } else {
            None
        };
        let endpoint = quinn::Endpoint::server(identity.server()?, config.listen)?;
        let diagnostics = Diagnostics::new(&config.state);
        Ok(Arc::new(Self {
            config,
            identity,
            endpoint,
            state: Mutex::new(State {
                peers,
                pending,
                quotas: HashMap::new(),
                connections: HashMap::new(),
                dialers: HashMap::new(),
                media: HashMap::new(),
            }),
            slots: Arc::new(Semaphore::new(64)),
            handshake_slots: Arc::new(Semaphore::new(16)),
            management_slots: Arc::new(Semaphore::new(16)),
            bulk_slots: Arc::new(Semaphore::new(8)),
            diagnostics,
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
        let pending = Some(Pending {
            token: token.clone(),
            expires,
            claimed: None,
        });
        let mut state = self.state.lock().unwrap();
        identity::save(&self.config.state.join("invitation.json"), &pending)?;
        state.pending = pending;
        drop(state);
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
        let mut pending = self
            .dial(
                &peer,
                json!({"op":"pair", "token":invite.token,"peer":self.local_peer()}),
                Instant::now() + TIMEOUT,
            )
            .await?;
        let connection = pending.connection.clone();
        self.persist_peer(peer.clone())?;
        self.register(&peer.device_id, &connection)?;
        pending.registered = true;
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
        if state
            .pending
            .as_ref()
            .and_then(|p| p.claimed.as_ref())
            .is_some_and(|(peer, _)| peer == id)
        {
            // Clear recovery before revoking trust: a crash cannot resurrect it.
            identity::save(
                &self.config.state.join("invitation.json"),
                &Option::<Pending>::None,
            )?;
            state.pending = None;
        }
        let mut peers = state.peers.clone();
        peers.remove(id);
        identity::save(&self.config.state.join("peers.json"), &peers)?;
        state.peers = peers;
        state.quotas.remove(id);
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
        self.diagnostics.record("service_started", None, None);
        let service = self.clone();
        let routes = tokio::spawn(async move { service.routes().await });
        let service = self.clone();
        let incoming = tokio::spawn(async move {
            while let Some(incoming) = service.endpoint.accept().await {
                let Ok(permit) = service.handshake_slots.clone().try_acquire_owned() else {
                    incoming.refuse();
                    continue;
                };
                let service = service.clone();
                tokio::spawn(async move {
                    match within(Instant::now() + TIMEOUT, "incoming TLS handshake", async {
                        incoming.await
                    })
                    .await
                    {
                        Ok(connection) => service.accept(connection, Some(permit)).await,
                        Err(error) => service.diagnostics.record(
                            "handshake_failed",
                            None,
                            Some(&format!("{error:#}")),
                        ),
                    }
                });
            }
        });
        loop {
            let (socket, _) = listener.accept().await?;
            if !same_user(&socket) {
                continue;
            }
            let Ok(permit) = self.management_slots.clone().try_acquire_owned() else {
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
    fn accept_pair(&self, peer: Peer, token: &str) -> Result<String> {
        peer.validate()?;
        ensure!(peer.device_id != self.id(), "cannot pair with self");
        let mut state = self.state.lock().unwrap();
        let mut pending = state.pending.clone().context("pairing disabled")?;
        ensure!(
            pending.expires >= now() && pending.token == token,
            "invalid invitation"
        );
        let claim = (peer.device_id.clone(), peer.certificate.clone());
        ensure!(
            pending
                .claimed
                .as_ref()
                .is_none_or(|previous| previous == &claim),
            "invitation already claimed"
        );
        ensure!(
            state
                .peers
                .get(&peer.device_id)
                .is_none_or(|old| old.certificate == peer.certificate),
            "device identity changed"
        );
        ensure!(
            state.peers.contains_key(&peer.device_id) || state.peers.len() < PEER_LIMIT,
            "too many peers"
        );
        pending.claimed = Some(claim);
        // Persist the claim first so a lost acknowledgment or restart can retry
        // the same authenticated identity, without authorizing a second peer.
        identity::save(&self.config.state.join("invitation.json"), &Some(&pending))?;
        state.pending = Some(pending);
        let id = peer.device_id.clone();
        let mut peers = state.peers.clone();
        peers.insert(id.clone(), peer);
        identity::save(&self.config.state.join("peers.json"), &peers)?;
        state.peers = peers;
        Ok(id)
    }
    async fn accept(
        self: Arc<Self>,
        connection: Connection,
        permit: Option<tokio::sync::OwnedSemaphorePermit>,
    ) {
        let result = tokio::time::timeout(TIMEOUT, async {
            let (mut send, mut recv) = connection.accept_bi().await?;
            let request = Packet::read_header(&mut recv).await?.value;
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
                    self.accept_pair(
                        peer,
                        request["token"].as_str().context("missing invitation")?,
                    )?
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
        drop(permit);
        match result {
            Ok(id) => {
                if let Ok(_permit) = self.slots.clone().try_acquire_owned() {
                    self.connected(connection, id).await;
                } else {
                    connection.close(1u32.into(), b"trusted connection limit");
                }
            }
            Err(error) => {
                self.diagnostics
                    .record("authentication_failed", None, Some(&format!("{error:#}")));
                connection.close(1u32.into(), b"authentication rejected");
            }
        }
    }
    async fn connection(self: &Arc<Self>, id: &str) -> Result<Connection> {
        // One budget includes queueing, TLS, stream credit and authentication.
        // Concurrent callers must not each wait for all earlier 15s attempts.
        let deadline = Instant::now() + TIMEOUT;
        let dialer = {
            let mut state = self.state.lock().unwrap();
            ensure!(state.peers.contains_key(id), "unpaired device");
            if let Some(connection) = state
                .connections
                .get(id)
                .and_then(|list| list.iter().rev().find(|c| c.close_reason().is_none()))
            {
                return Ok(connection.clone());
            }
            state
                .dialers
                .entry(id.to_owned())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _dial = within(deadline, "wait for peer dial", async {
            Ok::<_, anyhow::Error>(dialer.lock().await)
        })
        .await?;
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
        let mut pending = self
            .dial(&peer, json!({"op":"auth","device_id":self.id()}), deadline)
            .await?;
        let connection = pending.connection.clone();
        self.register(id, &connection)?;
        pending.registered = true;
        let service = self.clone();
        let remote = id.to_owned();
        let clone = connection.clone();
        tokio::spawn(async move {
            service.connected(clone, remote).await;
        });
        Ok(connection)
    }
    async fn dial(
        &self,
        peer: &Peer,
        request: Value,
        deadline: Instant,
    ) -> Result<PendingConnection> {
        let connection = within(
            deadline,
            "TLS handshake",
            self.endpoint.connect_with(
                self.identity.client(&peer.certificate)?,
                peer.address.parse()?,
                "lan-remoteapp",
            )?,
        )
        .await?;
        let pending = PendingConnection {
            connection,
            registered: false,
        };
        let connection = &pending.connection;
        ensure!(
            identity::remote_certificate(connection)? == peer.certificate,
            "certificate mismatch"
        );
        let (mut send, mut recv) =
            within(deadline, "open authentication stream", connection.open_bi()).await?;
        within(
            deadline,
            "write authentication",
            Packet::new(request, 0)?.write(&mut send),
        )
        .await?;
        ensure!(
            within(
                deadline,
                "read authentication",
                Packet::read_header(&mut recv)
            )
            .await?
            .value["ok"]
                == true,
            "authentication rejected"
        );
        send.finish()?;
        Ok(pending)
    }
    async fn connected(self: Arc<Self>, connection: Connection, id: String) {
        self.diagnostics.record(
            "connection_opened",
            Some(&id),
            Some(&connection.remote_address().to_string()),
        );
        let mut tasks = JoinSet::new();
        let reason = loop {
            tokio::select! {
                result = connection.accept_bi() => {
                    let (send,recv) = match result { Ok(streams) => streams, Err(error) => break error.to_string() };
                    if tasks.len() >= 128 || !self.trusted(&id,&identity::remote_certificate(&connection).unwrap_or_default()) { connection.close(1u32.into(),b"device limit or revoked"); break "device limit or revoked".into(); }
                    let service = self.clone(); let conn = connection.clone();
                    let peer = id.clone();
                    let quota = self.state.lock().unwrap().quotas.entry(id.clone()).or_default().clone();
                    tasks.spawn(async move {
                        if let Err(error) = service.clone().stream(conn,send,recv,quota).await {
                            service.diagnostics.record("stream_ended", Some(&peer), Some(&format!("{error:#}")));
                        }
                    });
                }
                _ = tasks.join_next(), if !tasks.is_empty() => {}
            }
        };
        self.diagnostics
            .record("connection_closed", Some(&id), Some(&reason));
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
        quota: Arc<DeviceQuota>,
    ) -> Result<()> {
        let head = tokio::time::timeout(TIMEOUT, Packet::read_header(&mut recv))
            .await??
            .value;
        match head["op"].as_str() {
            Some("rwp") => {
                let channel = head["channel"].as_str().context("missing channel")?;
                uuid::Uuid::parse_str(channel)?;
                let socket = UnixStream::connect(&self.config.publisher).await?;
                self.publish(connection, send, recv, socket, channel.to_owned(), quota)
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
                let media_stream = recv.id().index();
                let result = async {
                    loop {
                        let Some(mut packet) = Packet::read_media(&mut recv, &quota.media).await?
                        else {
                            return Ok(());
                        };
                        ensure!(
                            packet.value["type"] == "frame"
                                && packet.value["window_id"] == head["window_id"],
                            "invalid media"
                        );
                        packet.media_stream = Some(media_stream);
                        tokio::time::timeout(TIMEOUT, sender.send(packet)).await??;
                    }
                    #[allow(unreachable_code)]
                    Ok::<(), anyhow::Error>(())
                }
                .await;
                if result.is_err() {
                    let _ = sender.try_send(Packet::new(
                        json!({"type":"media.failed","window_id":head["window_id"]}),
                        0,
                    )?);
                }
                result
            }
            Some("bulk-upload" | "bulk-download") => {
                let _device = quota
                    .bulk
                    .clone()
                    .try_acquire_owned()
                    .context("device bulk limit")?;
                let _global = self
                    .bulk_slots
                    .clone()
                    .try_acquire_owned()
                    .context("bulk limit")?;
                self.bulk_server(send, recv, &head).await
            }
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
                    Ok(Value::Null)
                }
                _ => bail!("unknown management operation"),
            }
        }
        .await;
        if let Err(error) = &result {
            self.diagnostics
                .record("management_failed", None, Some(&format!("{error:#}")));
        }
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
impl Service {
    async fn publish(
        self: &Arc<Self>,
        connection: Connection,
        mut send: SendStream,
        mut recv: RecvStream,
        socket: UnixStream,
        channel: String,
        quota: Arc<DeviceQuota>,
    ) -> Result<()> {
        send.set_priority(10)?;
        let (read, mut write) = socket.into_split();
        let mut read = BufReader::new(read);
        let mut media: HashMap<u64, MediaQueue> = HashMap::new();
        let mut serial = 0u64;
        let mut workers = JoinSet::new();
        let mut barrier = 0u64;
        let (key_tx, mut key_rx) = mpsc::channel::<u64>(64);
        let input = async {
            loop {
                // Keep the partially read packet alive while servicing refreshes.
                let packet = Packet::read_control(&mut recv, &quota.control);
                tokio::pin!(packet);
                loop {
                    tokio::select! {
                        packet = &mut packet => {
                            let packet = packet?;
                            ensure!(packet.payload.is_empty() && allowed_request(&packet.value), "request not allowed");
                            tokio::time::timeout(TIMEOUT, framing::json(&mut write, &packet.value)).await??;
                            break;
                        }
                        Some(id) = key_rx.recv() => {
                            tokio::time::timeout(TIMEOUT, framing::json(&mut write, &json!({"version":1,"type":"video.keyframe","window_id":id}))).await??;
                        }
                    }
                }
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        };
        let mut refresh = tokio::time::interval(Duration::from_millis(500));
        let mut supports_refresh = false;
        let output = async {
            loop {
                // Timer/worker completion must not cancel a partial JSON line.
                let next = framing::line(&mut read);
                tokio::pin!(next);
                loop {
                    tokio::select! {
                        value = &mut next => {
                            let value = value?;
                            if value["type"] == "hello" {
                                supports_refresh = value["capabilities"].as_array().is_some_and(|caps| caps.iter().any(|c| c == "video.keyframe"));
                            }
                            if value["type"] == "frame" {
                                let window = value["window_id"].as_u64().context("missing window")?;
                                ensure!(media.contains_key(&window) || media.len() < 64, "too many active windows");
                                let slot = media.entry(window).or_default();
                                let packet = Packet::new(value,barrier)?;
                                if !slot.accepts(&packet) { break; }
                                if slot.sender.is_none() {
                                    serial = serial.checked_add(1).context("media generation overflow")?;
                                    slot.serial = serial;
                                    let generation = serial;
                                    let (tx,mut rx) = mpsc::channel::<Packet>(2); slot.sender = Some(tx);
                                    let connection = connection.clone(); let channel = channel.clone();
                                    slot.worker = Some(workers.spawn(async move {
                                        let result = async {
                                            let (mut send,_recv) = within(Instant::now()+TIMEOUT, "open media", connection.open_bi()).await?;
                                            send.set_priority(0)?;
                                            within(Instant::now()+TIMEOUT, "media header", Packet::new(json!({"op":"media","channel":channel,"window_id":window}),0)?.write(&mut send)).await?;
                                            while let Some(packet) = rx.recv().await { tokio::time::timeout(TIMEOUT,packet.write(&mut send)).await??; }
                                            send.finish()?; Ok::<_,anyhow::Error>(())
                                        }.await;
                                        (window, generation, result)
                                    }));
                                }
                                // Never wait on an individual window here. Lost P
                                // frames trigger keyframe gating, not broken GOPs.
                                slot.enqueue(packet, &quota.media);
                                ensure!(supports_refresh || !slot.needs_keyframe(), "publisher upgrade required for media recovery");
                            } else {
                                match value["type"].as_str() {
                                    Some("window.destroyed") => { if let Some(id) = value["window_id"].as_u64() { media.remove(&id); } },
                                    Some("session.exited") => media.clear(),
                                    Some("session.snapshot") => media.clear(),
                                    _ => {}
                                }
                                barrier = barrier.checked_add(1).context("sequence overflow")?;
                                tokio::time::timeout(TIMEOUT,Packet::new(value,barrier)?.write(&mut send)).await??;
                            }
                            break;
                        }
                        _ = refresh.tick() => {
                            if supports_refresh {
                                for (&id, slot) in &media {
                                    if slot.needs_keyframe() { let _ = key_tx.try_send(id); }
                                }
                            }
                        }
                        worker = workers.join_next(), if !workers.is_empty() => {
                            match worker.context("media worker ended")? {
                                Ok((window, generation, _result)) => {
                                    if let Some(slot) = media.get_mut(&window).filter(|s| s.serial == generation) { slot.failed(); }
                                },
                                Err(error) if error.is_cancelled() => {},
                                Err(error) => return Err(error.into()),
                            }
                        }
                    }
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
        let result = self.receive_route(peer, socket).await;
        if let Err(error) = &result {
            self.diagnostics
                .record("route_ended", Some(peer), Some(&format!("{error:#}")));
        }
        result
    }
    async fn receive_route(self: &Arc<Self>, peer: &str, socket: UnixStream) -> Result<()> {
        let (read, mut write) = socket.into_split();
        let mut read = BufReader::new(read);
        let (requests_tx, mut requests) = mpsc::channel(1);
        // Watch the local caller while dialing as well as while streaming.
        // A closed window/expired RPC cancels its queued attempt immediately.
        // Buffer a bounded number of requests without replaying any of them.
        let local_input = async {
            loop {
                let value = framing::line(&mut read).await?;
                requests_tx.send(value).await?;
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        };
        tokio::pin!(local_input);
        let channel = uuid::Uuid::new_v4().to_string();
        let deadline = Instant::now() + TIMEOUT;
        let setup = async {
            let connection = self.connection(peer).await?;
            let (mut send, recv) =
                within(deadline, "open control stream", connection.open_bi()).await?;
            send.set_priority(10)?;
            within(
                deadline,
                "write control header",
                Packet::new(json!({"op":"rwp","channel":channel}), 0)?.write(&mut send),
            )
            .await?;
            Ok::<_, anyhow::Error>((connection, send, recv))
        };
        let (connection, mut send, mut recv) = tokio::select! {
            result = &mut local_input => return result,
            result = setup => result?,
        };
        let quota = self
            .state
            .lock()
            .unwrap()
            .quotas
            .entry(peer.to_owned())
            .or_default()
            .clone();
        let (tx, mut rx) = mpsc::channel::<Packet>(8);
        let (control_tx, mut control_rx) = mpsc::channel::<Result<Packet>>(8);
        let key = (connection.stable_id(), channel.clone());
        self.state.lock().unwrap().media.insert(key.clone(), tx);
        let result = async {
            let input = async {
                loop {
                    let value = requests.recv().await.context("local input ended")?;
                    within(Instant::now()+TIMEOUT, "write control request", Packet::new(value,0)?.write(&mut send)).await?;
                }
                #[allow(unreachable_code)] Ok::<(),anyhow::Error>(())
            };
            let control = async {
                loop {
                    let packet = Packet::read_control(&mut recv, &quota.control).await;
                    let ended = packet.is_err();
                    control_tx.send(packet).await?;
                    // Deliver EOF after the final control event. Returning here
                    // races the output branch and can discard session.exited.
                    if ended { std::future::pending::<()>().await; }
                }
                #[allow(unreachable_code)] Ok::<(),anyhow::Error>(())
            };
            let output = async {
                let mut merge = Merge::default(); let mut pending: Vec<Packet> = Vec::new();
                loop {
                    tokio::select! {
                        biased;
                        packet = control_rx.recv() => {
                            let packet = packet.context("control ended")??; ensure!(packet.value["type"] != "frame" && packet.barrier == merge.barrier+1, "control ordering");
                            merge.barrier = packet.barrier; merge.observe(&packet.value)?;
                            let value = merge.deliver(packet); framing::json(&mut write,&value).await?;
                        }
                        packet = rx.recv(), if pending.len() < 8 => {
                            let packet = packet.context("media ended")?;
                            if packet.value["type"] == "media.failed" {
                                // A single media stream may reset independently.
                                // Its publisher restarts at an IDR; control and
                                // other windows must remain usable meanwhile.
                            } else {
                                ensure!(packet.value["type"] == "frame", "invalid media"); pending.push(packet);
                            }
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
            tokio::select! { r = &mut local_input => r, r = input => r, r = output => r, r = control => r }
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
        tokio::time::timeout(Duration::from_secs(1), async {
            framing::json(&mut write, &json!({"version":1,"type":"hello"})).await?;
            framing::line(&mut read).await?;
            check_sharing(&mut read, &mut write).await
        })
        .await??;
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
        let transfer = tokio::time::timeout(Duration::from_secs(300), async {
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
        });
        let result = tokio::select! {
            result = transfer => result.map_err(anyhow::Error::from).and_then(|r| r),
            result = watch_sharing(&mut read, &mut write) => result,
        };
        if result.is_err() {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        }
        result
    }
    async fn open_bulk(
        self: &Arc<Self>,
        request: &Value,
        deadline: Instant,
    ) -> Result<(SendStream, RecvStream)> {
        // Stream credit and header writes can stall even on an authenticated,
        // live connection. Bound all setup stages, not just the later body copy,
        // so a stalled peer cannot retain local management slots indefinitely.
        let connection = within(
            deadline,
            "connect bulk peer",
            self.connection(request["device_id"].as_str().context("device required")?),
        )
        .await?;
        let (mut send, recv) = within(deadline, "open bulk stream", connection.open_bi()).await?;
        send.set_priority(-10)?;
        within(
            deadline,
            "write bulk header",
            Packet::new(request.clone(), 0)?.write(&mut send),
        )
        .await?;
        Ok((send, recv))
    }
    async fn bulk_client<R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin>(
        self: &Arc<Self>,
        request: &Value,
        read: &mut R,
        write: &mut W,
    ) -> Result<()> {
        let (mut send, mut recv) = self.open_bulk(request, Instant::now() + TIMEOUT).await?;
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
    media_streams: HashMap<u64, u64>,
}
impl Merge {
    fn observe(&mut self, value: &Value) -> Result<()> {
        match value["type"].as_str() {
            Some("session.snapshot" | "session.exited") => {
                self.windows.clear();
                self.media_streams.clear();
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
                    if value["type"] == "window.destroyed" {
                        self.windows.remove(&id);
                        self.media_streams.remove(&id);
                        return Ok(());
                    }
                    if value["type"] == "window.metadata" && !self.windows.contains_key(&id) {
                        return Ok(());
                    }
                    ensure!(
                        self.windows.contains_key(&id) || self.windows.len() < 128,
                        "active window limit"
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
    fn current(&mut self, packet: &Packet) -> bool {
        let Some(id) = packet.value["window_id"].as_u64() else {
            return false;
        };
        if packet.barrier < self.reset
            || !self
                .windows
                .get(&id)
                .is_some_and(|(born, revision, hidden)| {
                    !hidden
                        && packet.barrier >= *born
                        && packet.value["geometry_revision"].as_u64() == Some(*revision)
                })
        {
            return false;
        }
        if let Some(stream) = packet.media_stream {
            // A restarted media stream begins at an IDR, but QUIC only orders
            // within each stream. Once its new frames are delivered, late old
            // stream P frames must never be spliced into the replacement GOP.
            let latest = self.media_streams.entry(id).or_insert(stream);
            if stream < *latest {
                return false;
            }
            *latest = stream;
        }
        true
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
                    | "desktop.list"
                    | "session.list"
                    | "video.enable"
                    | "video.configure"
                    | "video.feedback"
                    | "video.keyframe"
                    | "app.launch"
                    | "desktop.start"
                    | "session.resume"
                    | "session.detach"
                    | "window.focus"
                    | "window.close"
                    | "window.close.request"
                    | "window.configure"
                    | "window.page-scale"
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

async fn check_sharing<R: tokio::io::AsyncBufRead + Unpin, W: tokio::io::AsyncWrite + Unpin>(
    read: &mut R,
    write: &mut W,
) -> Result<()> {
    framing::json(write, &json!({"version":1,"type":"node.info"})).await?;
    ensure!(
        framing::line(read).await?["sharing_enabled"] == true,
        "sharing disabled"
    );
    Ok(())
}
async fn watch_sharing<R: tokio::io::AsyncBufRead + Unpin, W: tokio::io::AsyncWrite + Unpin>(
    read: &mut R,
    write: &mut W,
) -> Result<()> {
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        tokio::time::timeout(Duration::from_secs(1), check_sharing(read, write)).await??;
    }
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
    fn test_service() -> (tempfile::TempDir, Arc<Service>) {
        // macOS Unix socket paths must stay short; Windows has no /tmp.
        #[cfg(unix)]
        let root = tempfile::tempdir_in("/tmp").unwrap();
        #[cfg(not(unix))]
        let root = tempfile::tempdir().unwrap();
        let service = Service::new(Config {
            state: root.path().join("n"),
            publisher: root.path().join("p.sock"),
            listen: "127.0.0.1:0".parse().unwrap(),
            name: "test".into(),
            device_id: None,
            bulk_agent: None,
        })
        .unwrap();
        (root, service)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stalled_window_keeps_control_and_other_window_live_then_recovers_static_picture() {
        use base64::engine::general_purpose::STANDARD;
        let (_root, service) = test_service();
        let (_remote_root, remote) = test_service();
        let dialing = remote
            .endpoint
            .connect_with(
                remote
                    .identity
                    .client(&service.identity.certificate)
                    .unwrap(),
                service.address(),
                "lan-remoteapp",
            )
            .unwrap();
        let accepting = async { service.endpoint.accept().await.unwrap().await.unwrap() };
        let (client, server) = tokio::join!(dialing, accepting);
        let client = client.unwrap();
        let (mut requests, mut controls) = client.open_bi().await.unwrap();
        Packet::new(json!({"type":"hello","version":1}), 0)
            .unwrap()
            .write(&mut requests)
            .await
            .unwrap();
        let (send, recv) = server.accept_bi().await.unwrap();
        let (publisher, fixture) = UnixStream::pair().unwrap();
        let quota = Arc::new(DeviceQuota::default());
        let publishing = {
            let service = service.clone();
            let quota = quota.clone();
            tokio::spawn(async move {
                service
                    .publish(server, send, recv, publisher, "test".into(), quota)
                    .await
            })
        };
        let (refreshed, refresh_seen) = tokio::sync::oneshot::channel();
        let fixture = tokio::spawn(async move {
            let (read, mut write) = fixture.into_split();
            let mut read = BufReader::new(read);
            framing::line(&mut read).await.unwrap();
            framing::json(
                &mut write,
                &json!({"type":"hello","capabilities":["video.keyframe"]}),
            )
            .await
            .unwrap();
            for id in [1, 2] {
                framing::json(
                    &mut write,
                    &json!({"type":"window.created","window_id":id,"geometry_revision":1}),
                )
                .await
                .unwrap();
            }
            let data = STANDARD.encode(vec![0; 1024 * 1024]);
            for n in 0..12 {
                framing::json(&mut write,&json!({"type":"frame","window_id":1,"codec":"h264","data":data,"key_frame":n==0,"number":n})).await.unwrap();
            }
            framing::json(&mut write,&json!({"type":"frame","window_id":2,"codec":"h264","data":"YQ==","key_frame":true})).await.unwrap();
            framing::json(
                &mut write,
                &json!({"type":"clipboard.data","text":"control-live"}),
            )
            .await
            .unwrap();
            let mut refreshed = Some(refreshed);
            while let Ok(value) = framing::line(&mut read).await {
                if value["type"] == "video.keyframe" && value["window_id"] == 1 {
                    if let Some(notice) = refreshed.take() {
                        let _ = notice.send(());
                    }
                    framing::json(&mut write,&json!({"type":"frame","window_id":1,"codec":"h264","data":"YQ==","key_frame":true,"recovery":true})).await.unwrap();
                }
            }
        });
        let mut stalled = None;
        tokio::time::timeout(Duration::from_secs(8), async {
            let mut other = false;
            for _ in 0..2 {
                let (send, mut read) = client.accept_bi().await.unwrap();
                let header = Packet::read_header(&mut read).await.unwrap();
                if header.value["window_id"] == 1 {
                    stalled = Some((send, read));
                } else {
                    assert_eq!(Packet::read(&mut read).await.unwrap().value["window_id"], 2);
                    other = true;
                }
            }
            assert!(other);
            loop {
                if Packet::read(&mut controls).await.unwrap().value["text"] == "control-live" {
                    break;
                }
            }
            refresh_seen.await.unwrap();
        })
        .await
        .expect("control and the second window must progress while window 1 is unread");
        let (_, mut stalled) = stalled.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let packet = Packet::read(&mut stalled).await.unwrap();
                if packet.value["recovery"] == true {
                    assert_eq!(packet.value["key_frame"], true);
                    break;
                }
            }
        })
        .await
        .expect("a static picture must recover through an explicit IDR request");
        publishing.abort();
        let _ = publishing.await;
        fixture.abort();
        let _ = fixture.await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while quota.media.available_permits() != 64 * 1024 * 1024 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled media workers must release quota");
        client.close(0u32.into(), b"test complete");
    }

    #[tokio::test]
    async fn bulk_setup_deadline_covers_exhausted_stream_credit() {
        let (_root, service) = test_service();
        let (_remote_root, remote) = test_service();
        let peer = remote.local_peer();
        let mut config = remote.identity.server().unwrap();
        let mut transport = quinn::TransportConfig::default();
        transport.max_concurrent_bidi_streams(0u32.into());
        config.transport_config(Arc::new(transport));
        remote.endpoint.set_server_config(Some(config));
        service.persist_peer(peer.clone()).unwrap();
        let dialing = service
            .endpoint
            .connect_with(
                service.identity.client(&peer.certificate).unwrap(),
                remote.address(),
                "lan-remoteapp",
            )
            .unwrap();
        let accepting = async { remote.endpoint.accept().await.unwrap().await.unwrap() };
        let (client, server) = tokio::join!(dialing, accepting);
        let client = client.unwrap();
        // Authentication is already complete for this fixture: isolate the
        // subsequent bulk stream-credit wait from the handshake deadline.
        service.register(&peer.device_id, &client).unwrap();
        let request = json!({"op":"bulk-download","device_id":peer.device_id,"transfer_id":"copy"});
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            service.open_bulk(&request, Instant::now() + Duration::from_millis(100)),
        )
        .await
        .expect("bulk setup must not wait forever")
        .unwrap_err();
        assert!(error.to_string().contains("open bulk stream timed out"));
        assert!(
            client.close_reason().is_none(),
            "a stalled operation must not close the shared peer connection"
        );
        client.close(0u32.into(), b"test complete");
        server.close(0u32.into(), b"test complete");
    }

    #[tokio::test]
    async fn lost_pair_ack_can_retry_same_identity_but_not_after_revoke() {
        let (root, service) = test_service();
        let (_other_root, other) = test_service();
        let invitation: Invitation =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(service.invite().unwrap()).unwrap())
                .unwrap();
        let peer = other.local_peer();
        service
            .accept_pair(peer.clone(), &invitation.token)
            .unwrap();
        service
            .accept_pair(peer.clone(), &invitation.token)
            .unwrap();
        let persisted: Option<Pending> =
            identity::read(&root.path().join("n/invitation.json")).unwrap();
        assert_eq!(persisted.unwrap().claimed.unwrap().0, peer.device_id);
        let (_third_root, third) = test_service();
        assert!(service
            .accept_pair(third.local_peer(), &invitation.token)
            .is_err());
        service.revoke(&peer.device_id).unwrap();
        assert!(service.accept_pair(peer, &invitation.token).is_err());
    }
    #[tokio::test(start_paused = true)]
    async fn queued_dials_share_one_deadline_instead_of_serial_timeouts() {
        let (_root, service) = test_service();
        let (_remote_root, remote) = test_service();
        let peer = remote.local_peer();
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        let held = gate.lock().await;
        {
            let mut state = service.state.lock().unwrap();
            state.peers.insert(peer.device_id.clone(), peer.clone());
            state.dialers.insert(peer.device_id.clone(), gate.clone());
        }
        let mut tasks = JoinSet::new();
        for _ in 0..20 {
            let service = service.clone();
            let id = peer.device_id.clone();
            tasks.spawn(async move { service.connection(&id).await });
        }
        tokio::task::yield_now().await;
        tokio::time::advance(TIMEOUT + Duration::from_millis(1)).await;
        while let Some(result) = tasks.join_next().await {
            let error = result.unwrap().unwrap_err();
            assert!(error.to_string().contains("wait for peer dial timed out"));
        }
        drop(held);
        assert!(
            gate.try_lock().is_ok(),
            "expired waiters must release the gate"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn saturated_remote_admission_cannot_starve_local_management() {
        let (root, service) = test_service();
        let _untrusted = service
            .handshake_slots
            .clone()
            .acquire_many_owned(16)
            .await
            .unwrap();
        let _trusted = service.slots.clone().acquire_many_owned(64).await.unwrap();
        let running = service.clone();
        let task = tokio::spawn(async move { running.run().await });
        let socket = root.path().join("n/control.sock");
        tokio::time::timeout(Duration::from_secs(2), async {
            while !socket.exists() {
                tokio::task::yield_now().await;
            }
            let stream = UnixStream::connect(&socket).await.unwrap();
            let (read, mut write) = stream.into_split();
            framing::json(&mut write, &json!({"op":"status"}))
                .await
                .unwrap();
            let response = framing::line(&mut BufReader::new(read)).await.unwrap();
            assert_eq!(response["device_id"], service.id());
        })
        .await
        .expect("local status must remain usable under remote saturation");
        task.abort();
        service.endpoint.close(0u32.into(), b"test complete");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn closed_local_callers_cancel_queued_dials_without_waiting_for_timeout() {
        let (_root, service) = test_service();
        let (_remote_root, remote) = test_service();
        let peer = remote.local_peer();
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        let _held = gate.lock().await;
        {
            let mut state = service.state.lock().unwrap();
            state.peers.insert(peer.device_id.clone(), peer.clone());
            state.dialers.insert(peer.device_id.clone(), gate.clone());
        }
        let mut tasks = JoinSet::new();
        for _ in 0..20 {
            let (socket, mut caller) = UnixStream::pair().unwrap();
            let service = service.clone();
            let id = peer.device_id.clone();
            tasks.spawn(async move { service.receive(&id, socket).await });
            framing::json(&mut caller, &json!({"version":1,"type":"hello"}))
                .await
                .unwrap();
            drop(caller);
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(result) = tasks.join_next().await {
                assert!(result.unwrap().is_err());
            }
        })
        .await
        .expect("closed clients must not remain queued behind the dial gate");
        assert!(service.state.lock().unwrap().media.is_empty());
    }

    #[tokio::test]
    async fn stalled_auth_stream_is_closed_and_the_next_dial_recovers() {
        let (_root, service) = test_service();
        let (_remote_root, remote) = test_service();
        let peer = remote.local_peer();
        let mut config = remote.identity.server().unwrap();
        let mut transport = quinn::TransportConfig::default();
        transport.max_concurrent_bidi_streams(0u32.into());
        config.transport_config(Arc::new(transport));
        remote.endpoint.set_server_config(Some(config));
        service.persist_peer(peer.clone()).unwrap();
        let endpoint = remote.endpoint.clone();
        let stalled = tokio::spawn(async move {
            let connection = endpoint.accept().await.unwrap().await.unwrap();
            connection.closed().await
        });
        let result = service
            .dial(
                &peer,
                json!({"op":"auth","device_id":service.id()}),
                Instant::now() + Duration::from_secs(1),
            )
            .await;
        assert!(
            matches!(result, Err(e) if e.to_string().contains("open authentication stream timed out"))
        );
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), stalled)
                .await
                .unwrap()
                .unwrap(),
            quinn::ConnectionError::ApplicationClosed(_)
        ));
        assert!(!service.status()["peers"][0]["connected"].as_bool().unwrap());

        remote
            .endpoint
            .set_server_config(Some(remote.identity.server().unwrap()));
        remote.persist_peer(service.local_peer()).unwrap();
        let accepting = remote.clone();
        let server = tokio::spawn(async move {
            let connection = accepting.endpoint.accept().await.unwrap().await.unwrap();
            accepting.accept(connection, None).await;
        });
        let recovered =
            tokio::time::timeout(Duration::from_secs(3), service.connection(&peer.device_id))
                .await
                .unwrap()
                .unwrap();
        assert!(recovered.close_reason().is_none());
        assert!(service.status()["peers"][0]["connected"].as_bool().unwrap());
        recovered.close(0u32.into(), b"test complete");
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn retired_windows_do_not_consume_live_window_quota() {
        let mut merge = Merge::default();
        for id in 1..=512 {
            merge.barrier += 1;
            merge
                .observe(&json!({"type":"window.created","window_id":id,"geometry_revision":1}))
                .unwrap();
            merge.barrier += 1;
            merge
                .observe(&json!({"type":"window.destroyed","window_id":id}))
                .unwrap();
            assert!(merge.windows.is_empty());
        }
    }
    #[test]
    fn late_frames_cannot_revive_windows_or_cross_geometry_changes() {
        let mut merge = Merge {
            barrier: 1,
            ..Default::default()
        };
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
    #[test]
    fn replacement_media_stream_rejects_late_frames_from_older_streams() {
        let mut merge = Merge {
            barrier: 1,
            ..Default::default()
        };
        merge
            .observe(&json!({"type":"window.created","window_id":7,"geometry_revision":1}))
            .unwrap();
        let frame = |stream, key| {
            let mut packet = Packet::new(
                json!({"type":"frame","window_id":7,"geometry_revision":1,
                "codec":"h264","data":"YQ==","key_frame":key}),
                1,
            )
            .unwrap();
            packet.media_stream = Some(stream);
            packet
        };
        assert!(merge.current(&frame(1, true)));
        assert!(merge.current(&frame(1, false)));
        assert!(merge.current(&frame(5, true)));
        assert!(
            !merge.current(&frame(1, false)),
            "old P frames corrupt the new GOP"
        );
        assert!(
            !merge.current(&frame(1, true)),
            "old keyframes cannot roll back recovery"
        );
        assert!(merge.current(&frame(5, false)));
        let mut obsolete_geometry = frame(9, true);
        obsolete_geometry.value["geometry_revision"] = json!(0);
        assert!(!merge.current(&obsolete_geometry));
        assert!(
            merge.current(&frame(5, false)),
            "invalid frames cannot advance the stream generation"
        );
        merge
            .observe(&json!({"type":"window.created","window_id":8,"geometry_revision":1}))
            .unwrap();
        let mut other = frame(1, true);
        other.value["window_id"] = json!(8);
        assert!(
            merge.current(&other),
            "stream generations are scoped to a window"
        );
        merge.barrier = 2;
        merge
            .observe(&json!({"type":"window.destroyed","window_id":7}))
            .unwrap();
        assert!(
            !merge.media_streams.contains_key(&7),
            "retired windows must release tracking state"
        );
        merge.observe(&json!({"type":"session.exited"})).unwrap();
        assert!(merge.media_streams.is_empty());
    }

    #[tokio::test]
    async fn bulk_guard_observes_stop_sharing_without_waiting_for_transfer_eof() {
        let mut replies =
            BufReader::new(&b"{\"sharing_enabled\":true}\n{\"sharing_enabled\":false}\n"[..]);
        let mut requests = Vec::new();
        let start = Instant::now();
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            watch_sharing(&mut replies, &mut requests),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("sharing disabled"));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(requests.iter().filter(|&&b| b == b'\n').count(), 2);
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
