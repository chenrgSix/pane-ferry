use super::*;
use paneferry_platform::{ipc, storage};

#[tokio::test]
async fn receiver_binds_session_negotiates_fps_and_cancels_idle_reads() {
    let directory = tempfile::tempdir().unwrap();
    storage::private_directory(directory.path()).unwrap();
    let route = directory.path().join("fixture.sock");
    let listener = ipc::bind(&route).unwrap();
    let label = format!("test-{}", uuid::Uuid::new_v4());
    let session = uuid::Uuid::new_v4().to_string();
    entries().lock().unwrap().insert(
        label.clone(),
        Entry {
            group: "test".into(),
            target: None,
            session: session.clone(),
            route,
            fps: 60,
            epoch: 0,
            connection: None,
            on_close: None,
        },
    );
    let server_session = session.clone();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        assert!(ipc::same_user(&stream));
        let (input, mut output) = stream.into_split();
        let mut input = BufReader::new(input);
        assert_eq!(
            local_rpc::read(&mut input).await.unwrap(),
            json!({"type":"hello","version":1})
        );
        local_rpc::write(&mut output, &json!({"type":"hello","version":1,"capabilities":["native.h264","video.qos","input.key.logical"],"video_max_fps":30})).await.unwrap();
        assert_eq!(
            local_rpc::read(&mut input).await.unwrap(),
            json!({"type":"video.enable","version":1,"codec":"h264","max_fps":30,"feedback":true})
        );
        local_rpc::write(&mut output, &json!({"type":"video.ready","version":1}))
            .await
            .unwrap();
        assert_eq!(
            local_rpc::read(&mut input).await.unwrap(),
            json!({"type":"session.resume","version":1,"session_id":server_session})
        );
        local_rpc::write(&mut output, &json!({"type":"session.snapshot","version":1,"session_id":server_session,"windows":[{"window_id":7,"width":640,"height":480,"geometry_revision":1}]})).await.unwrap();
        let key = local_rpc::read(&mut input).await.unwrap();
        assert_eq!(
            key,
            json!({"type":"input.key.logical","version":1,"window_id":7,"key":"Return","pressed":true})
        );
        // Disconnect only closes the pipe; it must not send window.close.request.
        assert!(local_rpc::read(&mut input).await.is_err());
    });
    let opened = open(&label, None).await.unwrap();
    let epoch = opened["epoch"].as_u64().unwrap();
    assert_eq!(opened["max_fps"], 30);
    assert_eq!(opened["selected_fps"], 60);
    assert!(send(
        &label,
        epoch,
        json!({"type":"input.key.logical","window_id":8,"key":"Return","pressed":true})
    )
    .await
    .is_err());
    assert!(send(
        &label,
        epoch,
        json!({"type":"window.close.request","window_id":7})
    )
    .await
    .is_err());
    assert!(send(
        &label,
        epoch,
        json!({"type":"window.configure","window_id":7,"width":8192,"height":480})
    )
    .await
    .is_err());
    send(
        &label,
        epoch,
        json!({"type":"input.key.logical","window_id":7,"key":"Return","pressed":true}),
    )
    .await
    .unwrap();
    let pending_label = label.clone();
    let pending = tokio::spawn(async move { next(&pending_label, epoch).await });
    tokio::task::yield_now().await;
    disconnect(&label);
    assert!(tokio::time::timeout(Duration::from_secs(2), pending)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    assert!(send(
        &label,
        epoch,
        json!({"type":"input.key.logical","window_id":7,"key":"Return","pressed":true})
    )
    .await
    .is_err());
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
    remove(&label);
}
