//! Diagnostic archive only: wall/CPU checkpoints without recording any pixels.
use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};

fn clock(id: libc::clockid_t) -> u64 {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    assert_eq!(unsafe { libc::clock_gettime(id, &mut t) }, 0);
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}

pub struct CaptureProbe {
    window: u32,
    points: Vec<serde_json::Value>,
}
impl CaptureProbe {
    pub fn new(window: u32) -> Self {
        let mut s = Self {
            window,
            points: Vec::with_capacity(8),
        };
        s.mark("start");
        s
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
        let event = serde_json::json!({"event":"x11_capture", "window_id":self.window, "points":self.points});
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
