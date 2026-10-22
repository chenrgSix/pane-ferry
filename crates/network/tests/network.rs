use remoteapp_network::{
    framing,
    service::{Config, Service},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::{
    io::BufReader,
    net::{UnixListener, UnixStream},
};

async fn node(name: &str) -> (tempfile::TempDir, Arc<Service>) {
    let dir = tempfile::Builder::new()
        .prefix("rwn-")
        .tempdir_in("/tmp")
        .unwrap();
    let publisher = dir.path().join("p.sock");
    let listener = UnixListener::bind(&publisher).unwrap();
    let name = name.to_owned();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let name = name.clone();
            tokio::spawn(async move {
                let (read, mut write) = socket.into_split();
                let mut read = BufReader::new(read);
                while let Ok(value) = framing::line(&mut read).await {
                    let response = match value["type"].as_str() {
                        Some("hello") => json!({"version":1,"type":"hello"}),
                        Some("app.list") => json!({"version":1,"type":"app.list","name":name}),
                        Some("session.resume") => {
                            for event in [
                                json!({"type":"session.snapshot"}),
                                json!({"type":"window.created","window_id":1,"geometry_revision":1}),
                            ] {
                                let mut event = event;
                                event["seq"] = json!(1);
                                event["version"] = json!(1);
                                if framing::json(&mut write, &event).await.is_err() {
                                    return;
                                }
                            }
                            for n in 0..10 {
                                if framing::json(&mut write,&json!({"type":"frame","version":1,"seq":n+3,"window_id":1,"geometry_revision":1,"codec":"h264","data":"YWJj","stream_id":1,"frame":n})).await.is_err() { return; }
                                tokio::time::sleep(Duration::from_millis(5)).await;
                            }
                            json!({"type":"clipboard.data","version":1,"seq":13,"text":name})
                        }
                        _ => value,
                    };
                    if framing::json(&mut write, &response).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    let service = Service::new(Config {
        state: dir.path().join("n"),
        publisher,
        listen: "127.0.0.1:0".parse().unwrap(),
        name: "test".into(),
        device_id: None,
        bulk_agent: None,
    })
    .unwrap();
    let runner = service.clone();
    tokio::spawn(async move {
        runner.run().await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(25)).await;
    (dir, service)
}
async fn request(node: &Arc<Service>, peer: &str, body: serde_json::Value) -> serde_json::Value {
    let (a, b) = UnixStream::pair().unwrap();
    let node = node.clone();
    let peer = peer.to_owned();
    tokio::spawn(async move {
        let _ = node.receive(&peer, a).await;
    });
    let (read, mut write) = b.into_split();
    let mut read = BufReader::new(read);
    framing::json(&mut write, &body).await.unwrap();
    tokio::time::timeout(Duration::from_secs(4), framing::line(&mut read))
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test]
async fn pairing_bidirectional_three_devices_revoke_and_identity_pins() {
    let (_a, a) = node("A").await;
    let (_b, b) = node("B").await;
    let (_c, c) = node("C").await;
    let invite = a.invite().unwrap();
    b.pair(&invite, &a.address().to_string()).await.unwrap();
    assert!(
        c.pair(&invite, &a.address().to_string()).await.is_err(),
        "invitation must be one-use"
    );
    c.pair(&a.invite().unwrap(), &a.address().to_string())
        .await
        .unwrap();
    assert_eq!(
        request(&a, b.id(), json!({"type":"app.list","version":1})).await["name"],
        "B"
    );
    assert_eq!(
        request(&b, a.id(), json!({"type":"app.list","version":1})).await["name"],
        "A"
    );
    assert_eq!(
        request(&a, c.id(), json!({"type":"app.list","version":1})).await["name"],
        "C"
    );
    a.revoke(b.id()).unwrap();
    assert_eq!(
        request(&a, c.id(), json!({"type":"app.list","version":1})).await["name"],
        "C"
    );
    let (socket, _) = UnixStream::pair().unwrap();
    assert!(a.receive(b.id(), socket).await.is_err());
    assert!(
        b.pair(&c.invite().unwrap(), &a.address().to_string())
            .await
            .is_err(),
        "certificate must match invitation"
    );
}
#[tokio::test]
async fn independent_media_keeps_codec_order_and_metadata_before_frames() {
    let (_a, a) = node("A").await;
    let (_b, b) = node("B").await;
    a.pair(&b.invite().unwrap(), &b.address().to_string())
        .await
        .unwrap();
    let (x, y) = UnixStream::pair().unwrap();
    let service = a.clone();
    let peer = b.id().to_owned();
    tokio::spawn(async move {
        let _ = service.receive(&peer, x).await;
    });
    let (read, mut write) = y.into_split();
    let mut read = BufReader::new(read);
    framing::json(
        &mut write,
        &json!({"type":"session.resume","version":1,"session_id":"same"}),
    )
    .await
    .unwrap();
    let mut sequence = 0;
    let mut frames = Vec::new();
    let mut window = false;
    while frames.len() < 10 {
        let value = tokio::time::timeout(Duration::from_secs(4), framing::line(&mut read))
            .await
            .unwrap()
            .unwrap();
        let seq = value["seq"].as_u64().unwrap();
        assert!(seq > sequence);
        sequence = seq;
        if value["type"] == "window.created" {
            window = true;
        }
        if value["type"] == "frame" {
            assert!(window);
            assert_eq!(value["data"], "YWJj");
            frames.push(value["frame"].as_u64().unwrap());
        }
    }
    assert_eq!(frames, (0..10).collect::<Vec<_>>());
}
#[tokio::test]
async fn native_peer_cannot_change_local_sharing_or_prepare_ssh_routes() {
    let (_a, a) = node("A").await;
    let (_b, b) = node("B").await;
    a.pair(&b.invite().unwrap(), &b.address().to_string())
        .await
        .unwrap();
    for kind in ["sharing.set", "permissions.request", "peer.prepare"] {
        let (x, y) = UnixStream::pair().unwrap();
        let service = a.clone();
        let peer = b.id().to_owned();
        tokio::spawn(async move {
            let _ = service.receive(&peer, x).await;
        });
        let (read, mut write) = y.into_split();
        let mut read = BufReader::new(read);
        framing::json(&mut write, &json!({"type":kind,"version":1,"enabled":true}))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(4), framing::line(&mut read))
                .await
                .unwrap()
                .is_err()
        );
    }
}
