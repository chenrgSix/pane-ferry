//! Diagnostic archive only. Numeric frame identities/timings, never payloads.
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    sync::{Mutex, OnceLock},
};

pub fn now() -> u64 {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    #[cfg(target_os = "macos")]
    let clock = libc::CLOCK_UPTIME_RAW;
    #[cfg(not(target_os = "macos"))]
    let clock = libc::CLOCK_MONOTONIC;
    assert_eq!(unsafe { libc::clock_gettime(clock, &mut t) }, 0);
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}

pub fn frame(event: &str, value: &Value, timestamp_ns: u64, details: Value) {
    if value["type"] != "frame" {
        return;
    }
    static FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();
    let file = FILE.get_or_init(|| {
        Mutex::new(
            std::env::var_os("PANEFERRY_NETWORK_PROBE_TRACE").and_then(|mut p| {
                p.push(format!(".{}.jsonl", std::process::id()));
                OpenOptions::new()
                    .append(true)
                    .create(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(p)
                    .ok()
            }),
        )
    });
    let v = json!({"event":event,"timestamp_ns":timestamp_ns,"pid":std::process::id(),
        "clock_domain":if cfg!(target_os="macos") {"mac_uptime"} else {"linux_CLOCK_MONOTONIC"},
        "frame_id":value["frame_id"],"session_id":value["session_id"],"window_id":value["window_id"],
        "key_frame":value["key_frame"],"details":details});
    if let Some(file) = file.lock().unwrap().as_mut() {
        if let Ok(mut bytes) = serde_json::to_vec(&v) {
            bytes.push(b'\n');
            let _ = file.write_all(&bytes);
        }
    }
}

pub fn progress(connection: quinn::Connection, value: &Value, stream: u64, offset: u64) {
    if value["key_frame"] != true {
        return;
    }
    let value = value.clone();
    tokio::spawn(async move {
        let begin = now();
        for _ in 0..100 {
            let s = connection.stats();
            frame(
                "quic_progress",
                &value,
                now(),
                json!({"stream_id":stream,"message_offset":offset,"begin_ns":begin,
                    "cwnd":s.path.cwnd,"rtt_us":s.path.rtt.as_micros(),
                    "mtu":s.path.current_mtu,"tx_bytes":s.udp_tx.bytes,"rx_bytes":s.udp_rx.bytes,
                    "tx_datagrams":s.udp_tx.datagrams,"rx_datagrams":s.udp_rx.datagrams,
                    "lost_packets":s.path.lost_packets,"lost_bytes":s.path.lost_bytes,
                    "congestion_events":s.path.congestion_events,
                    "rx_acks":s.frame_rx.acks,"tx_data_blocked":s.frame_tx.data_blocked,
                    "tx_stream_data_blocked":s.frame_tx.stream_data_blocked,
                    "rx_max_data":s.frame_rx.max_data,"rx_max_stream_data":s.frame_rx.max_stream_data}),
            );
            if connection.close_reason().is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    });
}
