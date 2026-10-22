use anyhow::{bail, Context, Result};
use remoteapp_network::{
    framing,
    service::{Config, Service},
};
use serde_json::{json, Value};
use std::{collections::HashMap, path::PathBuf};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    net::UnixStream,
};
#[tokio::main]
async fn main() {
    let failed = run().await.is_err();
    if failed {
        eprintln!("remoteapp-network: operation failed (check pairing, address and local service)");
    }
    // Tokio's stdin read is not cancellable. A bridge may lose the network
    // while its parent keeps stdin open; don't let runtime shutdown wait for
    // that read and hold stdout open forever, hiding EOF from the receiver.
    std::process::exit(if failed { 1 } else { 0 });
}
async fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().context("command required")?;
    let mut options = HashMap::new();
    while let Some(key) = args.next() {
        if !key.starts_with("--") || options.contains_key(&key) {
            bail!("invalid arguments");
        }
        options.insert(key, args.next().context("missing value")?);
    }
    let state = options
        .get("--state")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".local/state/lan-remoteapp/network")
        });
    match command.as_str() {
        "serve" => {
            let config = Config {
                state,
                publisher: PathBuf::from(options.get("--publisher").context("publisher required")?),
                listen: options
                    .get("--listen")
                    .map(String::as_str)
                    .unwrap_or("0.0.0.0:42424")
                    .parse()?,
                name: options.get("--name").cloned().unwrap_or_else(|| {
                    if cfg!(target_os = "macos") {
                        "Mac".into()
                    } else {
                        "Ubuntu".into()
                    }
                }),
                device_id: options.get("--device-id").cloned(),
                bulk_agent: options.get("--bulk-agent").map(PathBuf::from),
            };
            Service::new(config)?.run().await
        }
        "bridge" => {
            let socket =
                UnixStream::connect(options.get("--socket").context("socket required")?).await?;
            let (mut read, mut write) = socket.into_split();
            let input = async {
                tokio::io::copy(&mut tokio::io::stdin(), &mut write).await?;
                write.shutdown().await?;
                Ok::<_, anyhow::Error>(())
            };
            let output = async {
                tokio::io::copy(&mut read, &mut tokio::io::stdout()).await?;
                Ok::<_, anyhow::Error>(())
            };
            tokio::select! { r = input => r, r = output => r }
        }
        "bulk-upload" | "bulk-download" => {
            let socket = UnixStream::connect(state.join("control.sock")).await?;
            let (mut read, mut write) = socket.into_split();
            framing::json(&mut write,&json!({"op":command,"device_id":options.get("--peer").context("peer required")?,"transfer_id":options.get("--transfer")})).await?;
            let input = async {
                tokio::io::copy(&mut tokio::io::stdin(), &mut write).await?;
                write.shutdown().await?;
                Ok::<_, anyhow::Error>(())
            };
            let output = async {
                remoteapp_network::service::receive_bulk(&mut read, &mut tokio::io::stdout(), false)
                    .await
            };
            tokio::try_join!(input, output)?;
            Ok(())
        }
        "status" | "invite" | "pair" | "revoke" | "address" => {
            let mut request = if command == "pair" || command == "address" || command == "revoke" {
                framing::line(&mut BufReader::new(tokio::io::stdin())).await?
            } else {
                json!({})
            };
            request["op"] = Value::String(command);
            let socket = UnixStream::connect(state.join("control.sock")).await?;
            let (read, mut write) = socket.into_split();
            framing::json(&mut write, &request).await?;
            let value = framing::line(&mut BufReader::new(read)).await?;
            if value.get("error").is_some() {
                bail!("management failed");
            }
            framing::json(&mut tokio::io::stdout(), &value).await
        }
        _ => bail!("unknown command"),
    }
}
