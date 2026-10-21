//! Compiled only into a temporary benchmark copy, never the installed Agent.
use remoteapp_protocol::Request;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex, OnceLock,
    },
};

pub fn now() -> u64 {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value) },
        0
    );
    value.tv_sec as u64 * 1_000_000_000 + value.tv_nsec as u64
}
static CAPTURES: OnceLock<Mutex<HashMap<usize, (u64, u64)>>> = OnceLock::new();
pub fn capture(pointer: usize, start: u64) {
    let mut captures = CAPTURES.get_or_init(Default::default).lock().unwrap();
    if captures.len() > 128 {
        captures.clear();
    }
    captures.insert(pointer, (start, now()));
}
pub fn frame(pointer: usize) -> Value {
    let (start, end) = CAPTURES
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(&pointer)
        .unwrap();
    json!({"capture_start":start,"capture_end":end,"encode_start":now()})
}
pub fn request(stage: &str, request: &Request) {
    static RECEIVED: AtomicUsize = AtomicUsize::new(0);
    static DISPATCHED: AtomicUsize = AtomicUsize::new(0);
    if matches!(
        request,
        Request::Text { .. }
            | Request::Key { pressed: true, .. }
            | Request::Pointer {
                button: 1,
                pressed: true,
                ..
            }
    ) {
        let counter = if stage == "received" {
            &RECEIVED
        } else {
            &DISPATCHED
        };
        eprintln!(
            "LATENCY {}",
            json!({"kind":stage,"count":counter.fetch_add(1, Ordering::Relaxed)+1,"ns":now()})
        );
    }
}
