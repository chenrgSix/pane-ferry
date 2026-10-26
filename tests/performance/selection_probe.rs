//! Diagnostic-only: copied into a git-archive build by agent_stage_probe.py.
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    sync::{Mutex, OnceLock},
};

#[derive(Clone, Default, Serialize)]
pub struct Stamp {
    pub id: u64,
    pub epoch: u64,
    #[serde(skip_serializing)]
    pub sink: usize,
    #[serde(skip_serializing)]
    pub buffer: usize,
    pub produced_ns: u64,
    pub ready_ns: u64,
    pub marker: Option<u64>,
    pub raw_id: u64,
    pub raw_ns: u64,
}
#[derive(Default)]
struct State {
    next: u64,
    epochs: HashMap<usize, u64>,
    buffers: HashMap<usize, Stamp>,
    raw: HashMap<u64, Stamp>,
    latest: HashMap<usize, Stamp>,
    frames: HashMap<usize, Stamp>,
    publications: HashMap<usize, Value>,
}
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
thread_local! {
    static PRODUCED: Cell<(usize,usize)> = const {Cell::new((0,0))};
    static SELECTED: RefCell<Option<Stamp>> = const {RefCell::new(None)};
    static DEADLINE: Cell<u64> = const {Cell::new(0)};
    static INTERVAL: Cell<u64> = const {Cell::new(0)};
}
fn state() -> &'static Mutex<State> {
    STATE.get_or_init(Default::default)
}
pub fn sink(pointer: usize) {
    let mut s = state().lock().unwrap();
    s.next += 1;
    let epoch = s.next;
    s.epochs.insert(pointer, epoch);
    s.latest.remove(&pointer);
}
pub fn produced(sink: usize, buffer: usize, raw: bool, marker: Option<u64>, raw_ns: u64) {
    let mut s = state().lock().unwrap();
    s.next += 1;
    let origin = s.raw.get(&raw_ns).cloned().unwrap_or_default();
    let stamp = Stamp {
        id: s.next,
        epoch: *s.epochs.get(&sink).unwrap(),
        sink,
        buffer,
        produced_ns: crate::stage_probe::now(),
        ready_ns: 0,
        marker,
        raw_id: origin.id,
        raw_ns: origin.produced_ns,
    };
    if s.buffers.len() > 4096 {
        s.buffers.clear();
    }
    if raw {
        if s.raw.len() > 4096 {
            s.raw.clear();
        }
        s.raw.insert(stamp.produced_ns, stamp.clone());
    }
    s.buffers.insert(buffer, stamp);
    PRODUCED.set((sink, buffer));
}
pub fn ready(sink: usize) {
    let (producer, buffer) = PRODUCED.get();
    if producer != sink {
        return;
    }
    let mut s = state().lock().unwrap();
    if let Some(stamp) = s.buffers.get_mut(&buffer) {
        stamp.ready_ns = crate::stage_probe::now();
        let stamp = stamp.clone();
        s.latest.insert(sink, stamp);
    }
}
pub fn raw_timestamp(buffer: usize) -> Option<u64> {
    state()
        .lock()
        .unwrap()
        .buffers
        .get(&buffer)
        .map(|s| s.produced_ns)
}
pub fn selected(buffer: usize) {
    SELECTED.set(state().lock().unwrap().buffers.get(&buffer).cloned());
}
pub fn capture(pointer: usize) {
    if let Some(stamp) = SELECTED.with_borrow(Clone::clone) {
        let mut s = state().lock().unwrap();
        if s.frames.len() > 4096 {
            s.frames.clear();
        }
        s.frames.insert(pointer, stamp);
    }
}
pub fn deadline(age_ns: u64, interval_ns: u64) {
    DEADLINE.set(crate::stage_probe::now().saturating_sub(age_ns) + interval_ns);
    INTERVAL.set(interval_ns);
}
fn latest(s: &State, selected: &Stamp) -> Value {
    s.latest
        .get(&selected.sink)
        .filter(|v| v.epoch == selected.epoch)
        .map_or(Value::Null, |v| json!(v))
}
pub fn publish(pointer: usize) {
    let mut s = state().lock().unwrap();
    let Some(selected) = s.frames.get(&pointer).cloned() else {
        return;
    };
    let latest = latest(&s, &selected);
    let t = crate::stage_probe::now();
    let newer = latest["id"].as_u64().is_some_and(|id| id > selected.id);
    let newer_response = newer
        && selected
            .marker
            .zip(latest["marker"].as_u64())
            .is_some_and(|(a, b)| b > a);
    let v = json!({"selected":selected,"latest_at_publish":latest,"publish_ns":t,
        "eligible_ns":DEADLINE.get(),"interval_ns":INTERVAL.get(),
        "newer_ready":newer,"newer_response_ready":newer_response});
    if s.publications.len() > 4096 {
        s.publications.clear();
    }
    s.publications.insert(pointer, v.clone());
    drop(s);
    trace(json!({"event":"publish","selection":v}));
}
pub fn encoding(pointer: usize, t: u64) {
    let mut s = state().lock().unwrap();
    let Some(selected) = s.frames.get(&pointer).cloned() else {
        return;
    };
    let latest = latest(&s, &selected);
    if let Some(v) = s.publications.get_mut(&pointer) {
        v["encode_start_ns"] = json!(t);
        v["latest_at_encode"] = latest;
    }
}
pub fn frame(pointer: usize) -> Value {
    state()
        .lock()
        .unwrap()
        .publications
        .get(&pointer)
        .cloned()
        .unwrap_or(Value::Null)
}
pub fn sent(value: &Value, bytes: usize, start: u64, end: u64) {
    let s = &value["_agent_stages"];
    if !s.is_object() {
        return;
    }
    trace(
        json!({"event":"sent","stages":s,"selection":s["selection"],"encode_start":s["encode_start"],
        "encode_end":s["encode_end"],"write_begin":start,"write_end":end,"bytes":bytes,
        "feedback_id":value["frame_id"],"window_id":value["window_id"]}),
    );
}
fn trace(v: Value) {
    static FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();
    let f = FILE.get_or_init(|| {
        Mutex::new(std::env::var_os("PANEFERRY_PROBE_TRACE").and_then(|p| {
            OpenOptions::new()
                .append(true)
                .create(true)
                .mode(0o600)
                .open(p)
                .ok()
        }))
    });
    if let Some(f) = f.lock().unwrap().as_mut() {
        let _ = serde_json::to_writer(&mut *f, &v);
        let _ = f.write_all(b"\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_reuse_and_sink_epochs_preserve_exact_selected_frame() {
        sink(11);
        produced(11, 101, true, None, 0);
        let raw_ns = raw_timestamp(101).unwrap();
        let raw_id = state().lock().unwrap().buffers[&101].id;
        sink(12);
        produced(12, 201, false, Some(1), raw_ns);
        ready(12);
        selected(201);
        capture(301);
        deadline(0, 33_333_000);

        // The same allocation can be reused while an older Arc is selected.
        // A produced buffer is eligible only once appsink reports readiness.
        produced(12, 201, false, Some(2), raw_ns);
        ready(11);
        publish(301);
        let first = frame(301);
        assert_eq!(first["newer_ready"], false);
        assert_eq!(first["selected"]["marker"], 1);
        assert_eq!(first["selected"]["raw_id"], raw_id);
        assert!(first["selected"].get("buffer").is_none());
        assert!(first["selected"].get("sink").is_none());

        ready(12);
        publish(301);
        encoding(301, crate::stage_probe::now());
        let newer = frame(301);
        assert_eq!(newer["newer_ready"], true);
        assert_eq!(newer["newer_response_ready"], true);
        assert_eq!(newer["selected"]["id"], first["selected"]["id"]);
        assert_eq!(newer["latest_at_encode"]["marker"], 2);

        // Recreated converter addresses do not make an old geometry's buffer
        // a candidate for the new converter, or vice versa.
        sink(12);
        produced(12, 202, false, Some(3), raw_ns);
        ready(12);
        publish(301);
        let recreated = frame(301);
        assert!(recreated["latest_at_publish"].is_null());
        assert_eq!(recreated["newer_ready"], false);
    }
}
