//! Reverse SSH stream-local routes. No listening TCP port or Mac SSH server.
use anyhow::{bail, Context, Result};
use remoteapp_core::node::private_directory;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{BufRead, Write},
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    path::Path,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Offer {
    device_id: String,
    name: String,
    generation: String,
}
fn valid(value: &str) -> bool {
    value.len() == 36 && value.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-')
}
pub fn prepare(runtime: &Path, id: &str) -> Result<serde_json::Value> {
    if !valid(id) {
        bail!("invalid device id")
    }
    let directory = runtime.join("peers");
    private_directory(&directory)?;
    let generation = crate::random_id()?;
    let socket = directory.join(format!("p{generation}.sock"));
    if socket.as_os_str().len() >= 104 {
        bail!("peer socket path too long")
    }
    Ok(serde_json::json!({"type":"peer.prepared","socket":socket,"generation":generation}))
}
pub fn hold(runtime: &Path) -> Result<()> {
    let mut input = std::io::stdin().lock();
    let line =
        remoteapp_protocol::read_line_limit(&mut input, 2048)?.context("peer offer required")?;
    let offer: Offer = serde_json::from_slice(&line)?;
    if !valid(&offer.device_id)
        || offer.generation.len() != 32
        || !offer.generation.bytes().all(|c| c.is_ascii_hexdigit())
        || offer.name.is_empty()
        || offer.name.len() > 256
        || offer.name.chars().any(char::is_control)
    {
        bail!("invalid peer offer")
    }
    let directory = runtime.join("peers");
    private_directory(&directory)?;
    let socket = directory.join(format!("p{}.sock", offer.generation));
    let metadata = fs::symlink_metadata(&socket)?;
    if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
        bail!("invalid peer socket")
    }
    let record = directory.join(format!("{}.json", offer.device_id));
    let temporary = directory.join(format!(".{}.tmp", offer.generation));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let data = serde_json::json!({"version":1,"device_id":offer.device_id,"name":offer.name,
        "generation":offer.generation,"socket":socket,"transport":"ssh-reverse"});
    file.write_all(&serde_json::to_vec(&data)?)?;
    file.sync_all()?;
    fs::rename(&temporary, &record)?;
    println!("ready");
    std::io::stdout().flush()?;
    // EOF means this exact SSH route ended. An older route must never remove a
    // replacement offer from a reconnecting or independently connected device.
    loop {
        match input.fill_buf() {
            Ok([]) | Err(_) => break,
            Ok(bytes) => {
                let count = bytes.len();
                input.consume(count);
            }
        }
    }
    if fs::read(&record)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|value| value["generation"] == offer.generation)
    {
        let _ = fs::remove_file(record);
    }
    let _ = fs::remove_file(socket);
    Ok(())
}
