use anyhow::{bail, ensure, Context, Result};
use paneferry_network::framing;
#[cfg(windows)]
use paneferry_network::service::{Config, Service};
use paneferry_platform::ipc;
#[cfg(windows)]
use paneferry_platform::storage;
use serde_json::{json, Value};
use std::path::PathBuf;
#[cfg(windows)]
use std::sync::Arc;
use tokio::io::BufReader;
#[cfg(windows)]
use tokio::sync::Semaphore;

fn default_state() -> Result<PathBuf> {
    Ok(
        PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is unavailable")?)
            .join("PaneFerry"),
    )
}
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        // Only local CLI diagnostics; never print an incoming request or invitation.
        eprintln!("PaneFerry Windows: {error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    if command == "help" || command == "--help" {
        println!("PaneFerry Windows publisher\n  serve --share [--state DIRECTORY] [--listen IP:42424]\n  status | invite | pair | revoke | address [--state DIRECTORY]\n  sharing on|off [--state DIRECTORY]\nPair/revoke/address read a JSON object from stdin. Sharing is disabled unless --share is supplied.\nThe first version exposes existing single windows in the logged-in desktop only.");
        return Ok(());
    }
    let mut state = None;
    let mut listen = "0.0.0.0:42424".to_string();
    let mut share = false;
    let mut enabled = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--state" => {
                ensure!(state.is_none(), "duplicate state");
                state = Some(PathBuf::from(args.next().context("missing state")?));
            }
            "--listen" => listen = args.next().context("missing listen address")?,
            "--share" => share = true,
            "on" if command == "sharing" => enabled = Some(true),
            "off" if command == "sharing" => enabled = Some(false),
            _ => bail!("unsupported argument"),
        }
    }
    let root = match state {
        Some(path) => path,
        None => default_state()?,
    };
    ensure!(root.is_absolute(), "state directory must be absolute");
    let node = root.join("node");
    let network = root.join("network");
    match command.as_str() {
        "serve" => {
            #[cfg(not(windows))]
            {
                let _ = (node, network, listen, share);
                bail!("publishing requires Windows 11 with an unlocked desktop");
            }
            #[cfg(windows)]
            {
                storage::private_directory(&root)?;
                storage::private_directory(&node)?;
                storage::private_directory(&network)?;
                let lock_path = node.join("service.lock");
                ensure!(!lock_path.is_symlink(), "invalid service lock");
                let lock = storage::options()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(lock_path)?;
                lock.try_lock().context("publisher already running")?;
                let device = paneferry_core::node::device_id(&node)?;
                let publisher = ipc::bind(&node.join("node.sock"))?;
                let management = ipc::bind(&node.join("management.sock"))?;
                let state = paneferry_windows::State::new(device.clone(), share);
                let service = Service::new(Config {
                    state: network,
                    publisher: node.join("node.sock"),
                    listen: listen.parse()?,
                    name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows".into()),
                    device_id: Some(device),
                    bulk_agent: None,
                })?;
                let slots = Arc::new(Semaphore::new(16));
                fn factory() -> Result<Box<dyn paneferry_backend_windows::WindowSystem>> {
                    Ok(Box::new(paneferry_backend_windows::Desktop::new()?))
                }
                println!("PaneFerry Windows publisher running; sharing {}. Ctrl+C stops sharing and keeps applications open.",if share{"enabled"}else{"disabled"});
                tokio::select! {
                    result=service.run()=>result?,
                    result=paneferry_windows::serve(publisher,state.clone(),false,factory,slots.clone())=>result?,
                    result=paneferry_windows::serve(management,state.clone(),true,factory,slots.clone())=>result?,
                    _=tokio::signal::ctrl_c()=>{
                        state.lock().unwrap().stop();
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(6), slots.clone().acquire_many_owned(16)).await;

                    },
                }
                // Services own no application process. Runtime shutdown cannot kill published apps.
                drop(lock);
                std::process::exit(0);
            }
        }
        "sharing" => {
            let enabled = enabled.context("use sharing on or sharing off")?;
            let socket = ipc::LocalStream::connect(node.join("management.sock")).await?;
            let (read, mut write) = socket.into_split();
            let mut read = BufReader::new(read);
            framing::json(&mut write, &json!({"type":"hello","version":1})).await?;
            framing::line(&mut read).await?;
            framing::json(
                &mut write,
                &json!({"type":"sharing.set","version":1,"enabled":enabled}),
            )
            .await?;
            let response = framing::line(&mut read).await?;
            ensure!(
                response["type"] == "sharing.changed",
                "sharing change failed"
            );
            println!("Sharing {}", if enabled { "enabled" } else { "disabled" });
        }
        "status" | "invite" | "pair" | "revoke" | "address" => {
            let mut request = if matches!(command.as_str(), "pair" | "revoke" | "address") {
                framing::line(&mut BufReader::new(tokio::io::stdin())).await?
            } else {
                json!({})
            };
            ensure!(request.is_object(), "expected JSON object");
            request["op"] = Value::String(command);
            let socket = ipc::LocalStream::connect(network.join("control.sock")).await?;
            let (read, mut write) = socket.into_split();
            framing::json(&mut write, &request).await?;
            let response = framing::line(&mut BufReader::new(read)).await?;
            ensure!(response.get("error").is_none(), "network operation failed");
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        _ => bail!("unknown command; use --help"),
    }
    Ok(())
}
