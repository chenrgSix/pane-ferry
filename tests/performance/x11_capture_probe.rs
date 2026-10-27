//! Diagnostic archive only: wall/CPU checkpoints without recording any pixels.
use std::{
    cell::RefCell,
    collections::HashMap,
    fs::OpenOptions,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};

static QUEUED: OnceLock<Mutex<HashMap<u32, u64>>> = OnceLock::new();
thread_local! {
    static DETAILS: RefCell<Option<Vec<serde_json::Value>>> = const { RefCell::new(None) };
}
pub fn detail(stage: &str) {
    DETAILS.with_borrow_mut(|points| {
        if let Some(points) = points {
            points.push(serde_json::json!({"stage":stage,
                "timestamp_ns":clock(libc::CLOCK_MONOTONIC),
                "thread_cpu_ns":clock(libc::CLOCK_THREAD_CPUTIME_ID)}));
        }
    });
}
pub fn queued(window: u32) {
    let mut jobs = QUEUED.get_or_init(Default::default).lock().unwrap();
    if jobs.len() > 64 {
        jobs.clear();
    }
    jobs.insert(window, clock(libc::CLOCK_MONOTONIC));
}

pub fn scheduler(phase: &str, window: u32) {
    let Some(path) = std::env::var_os("PANEFERRY_PROBE_TRACE") else {
        return;
    };
    let value = serde_json::json!({"event":"x11_scheduler", "phase":phase,
        "window_id":window, "timestamp_ns":clock(libc::CLOCK_MONOTONIC),
        "thread_cpu_ns":clock(libc::CLOCK_THREAD_CPUTIME_ID)});
    if let Ok(mut bytes) = serde_json::to_vec(&value) {
        bytes.push(b'\n');
        if let Ok(mut file) = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(path)
        {
            let _ = file.write_all(&bytes);
        }
    }
}

fn clock(id: libc::clockid_t) -> u64 {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    assert_eq!(unsafe { libc::clock_gettime(id, &mut t) }, 0);
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}

pub struct CaptureProbe {
    id: u64,
    window: u32,
    queued_ns: Option<u64>,
    image_data: Option<usize>,
    marker: Option<u64>,
    points: Vec<serde_json::Value>,
}
impl CaptureProbe {
    pub fn new(window: u32) -> Self {
        DETAILS.set(Some(Vec::with_capacity(16)));
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let mut s = Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            window,
            queued_ns: QUEUED
                .get_or_init(Default::default)
                .lock()
                .unwrap()
                .remove(&window),
            image_data: None,
            marker: None,
            points: Vec::with_capacity(8),
        };
        s.mark("start");
        s
    }
    pub fn image(&mut self, image: &crate::CapturedImage, target: (u32, u32)) {
        self.image_data = Some(image.data.as_ptr() as usize);
        // Only the opt-in counter fixture uses these eight diagnostic squares.
        // No pixels or arbitrary user text are recorded.
        if image.bpp != 32 || image.pad != 32 || !image.little || target.0 == 0 || target.1 == 0 {
            return;
        }
        let scale = image.width / target.0;
        if scale == 0
            || image.width != target.0 * scale
            || image.height != target.1 * scale
            || image.width <= 241 * scale
            || image.height <= 16 * scale
        {
            return;
        }
        let mut marker = 0;
        for bit in 0..8 {
            let offset = ((16 * scale) as usize * image.width as usize
                + (16 + bit * 32) as usize * scale as usize)
                * 4;
            let Some(value) = image.data.get(offset) else {
                return;
            };
            if *value > 128 {
                marker |= 1 << bit;
            }
        }
        self.marker = Some(marker);
    }
    pub fn mark(&mut self, stage: &str) {
        self.points.push(serde_json::json!({
            "stage": stage,
            "timestamp_ns": clock(libc::CLOCK_MONOTONIC),
            "thread_cpu_ns": clock(libc::CLOCK_THREAD_CPUTIME_ID),
        }));
    }
}
impl Drop for CaptureProbe {
    fn drop(&mut self) {
        let Some(path) = std::env::var_os("PANEFERRY_PROBE_TRACE") else {
            return;
        };
        let event = serde_json::json!({"event":"x11_capture", "readback_id":self.id, "window_id":self.window,
            "queued_ns":self.queued_ns, "image_data":self.image_data, "marker":self.marker, "points":self.points,
            "details":DETAILS.take().unwrap_or_default()});
        if let Ok(mut bytes) = serde_json::to_vec(&event) {
            bytes.push(b'\n');
            if let Ok(mut file) = OpenOptions::new()
                .append(true)
                .create(true)
                .mode(0o600)
                .open(path)
            {
                let _ = file.write_all(&bytes);
            }
        }
    }
}
