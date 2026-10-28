//! Encoders are owned by two fixed workers, never by the control/socket writer.
//! A window stays on one worker so H.264 references remain ordered.
use super::{Pending, Shared};
#[path = "video_choice.rs"]
mod choice;
use anyhow::{bail, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use paneferry_protocol::Window;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
};

pub(super) struct Job {
    pub window: Window,
    pub image: Arc<crate::video::Frame>,
    pub session: String,
    pub generation: u64,
    pub fps: u8,
}
pub(super) struct Encoded {
    pub window: u32,
    pub generation: u64,
    pub value: Value,
    pub session: String,
}
pub(super) trait Codec {
    fn encode(&mut self, job: &Job) -> Result<Option<Value>>;
    fn forget(&mut self, id: u32);
    fn reset(&mut self);
}
pub(super) type Factory = Arc<dyn Fn() -> Box<dyn Codec> + Send + Sync>;
struct Cached {
    generation: u64,
    encoder: crate::video::Encoder,
    image: Option<Arc<crate::video::Frame>>,
    choice: choice::Choice,
}
struct RuntimeCodec {
    streams: Arc<AtomicU64>,
    windows: HashMap<u32, Cached>,
    gpu_failed: bool,
}
pub(super) fn factory() -> Factory {
    let streams = Arc::new(AtomicU64::new(1));
    Arc::new(move || {
        Box::new(RuntimeCodec {
            streams: streams.clone(),
            windows: HashMap::new(),
            gpu_failed: false,
        })
    })
}
impl Codec for RuntimeCodec {
    fn encode(&mut self, job: &Job) -> Result<Option<Value>> {
        let id = job.window.window_id;
        if self.windows.get(&id).is_some_and(|old| {
            old.generation != job.generation
                || old.encoder.width != job.image.width
                || old.encoder.height != job.image.height
                || old.encoder.format != job.image.format()
        }) {
            self.windows.remove(&id);
        }
        if !self.windows.contains_key(&id) {
            if self.windows.len() >= 64 {
                bail!("encoder window limit")
            }
            let choice = choice::Choice::new(
                job.image.is_gpu()
                    && !self.gpu_failed
                    && std::env::var("PANEFERRY_VIDEO_GPU").as_deref() != Ok("1"),
            );
            let software = self.gpu_failed || choice.starts_software();
            self.windows.insert(
                id,
                Cached {
                    generation: job.generation,
                    encoder: (if software {
                        crate::video::Encoder::software
                    } else {
                        crate::video::Encoder::for_format
                    })(
                        job.image.width,
                        job.image.height,
                        self.streams.fetch_add(1, Ordering::Relaxed),
                        job.fps,
                        job.image.format(),
                    )?,
                    image: None,
                    choice,
                },
            );
        }
        let encode_started = std::time::Instant::now();
        let dedup = if job.image.is_gpu() {
            if self.windows[&id].encoder.uses_gpu() {
                None
            } else {
                job.image.software_dedup_frame()?.map(Arc::new)
            }
        } else {
            Some(job.image.clone())
        };
        if self.windows[&id]
            .image
            .as_ref()
            .is_some_and(|old| dedup.as_ref().is_some_and(|image| old.same_pixels(image)))
        {
            return Ok(None);
        }
        // At most 8 MP of deduplication images per worker, independent of the
        // pending/in-flight frame budgets. Dropping cache does not drop H.264.
        if self
            .windows
            .values()
            .filter_map(|w| w.image.as_ref())
            .map(|i| i.pixels())
            .sum::<usize>()
            + job.image.pixels()
            > 8_000_000
        {
            for window in self.windows.values_mut() {
                window.image = None;
            }
        }
        let state = self.windows.get_mut(&id).unwrap();
        let packet = match state.encoder.encode_frame(&job.image) {
            Ok(packet) => packet,
            Err(_) if state.encoder.uses_gpu() => {
                // A failed GPU stream restarts software with a fresh IDR and
                // stream ID. Never append a new codec's references to its GOP.
                state.encoder = crate::video::Encoder::software(
                    job.image.width,
                    job.image.height,
                    self.streams.fetch_add(1, Ordering::Relaxed),
                    state.encoder.fps(),
                    job.image.format(),
                )?;
                state.encoder.encode_frame(&job.image)?
            }
            Err(error) => return Err(error),
        };
        let packet_stream = state.encoder.stream;
        self.gpu_failed |=
            job.image.is_gpu() && state.choice.attempted_gpu() && !state.encoder.uses_gpu();
        if let Some(gpu) = job.image.capture_elapsed_ms().and_then(|ms| {
            state.choice.observe(
                ms,
                job.image
                    .capture_ms()
                    .unwrap_or(0.0)
                    .max(encode_started.elapsed().as_secs_f64() * 1000.0),
                state.encoder.uses_gpu(),
            )
        }) {
            let create = if gpu && !self.gpu_failed {
                crate::video::Encoder::for_format
            } else {
                crate::video::Encoder::software
            };
            state.encoder = create(
                job.image.width,
                job.image.height,
                self.streams.fetch_add(1, Ordering::Relaxed),
                job.fps,
                job.image.format(),
            )?;
        }
        if self.gpu_failed || state.choice.software_selected() {
            job.image.prefer_software();
        }
        // Software snapshots retain owned CPU bytes, never a target BO. GPU
        // encoding stays damage-gated without forcing a CPU comparison.
        state.image = dedup;
        Ok(Some(
            json!({"type":"frame", "window_id":id, "geometry_revision":job.window.geometry_revision,
            "width":job.window.width, "height":job.window.height, "codec":"h264", "data":STANDARD.encode(packet.bytes),
            "stream_id":packet_stream, "key_frame":packet.keyframe,
            "encoded_width":job.image.width + job.image.width % 2, "encoded_height":job.image.height + job.image.height % 2}),
        ))
    }
    fn forget(&mut self, id: u32) {
        self.windows.remove(&id);
    }
    fn reset(&mut self) {
        self.windows.clear();
    }
}
enum Work {
    Frame(Job),
    Forget(u32),
    Reset,
}
impl Work {
    fn pixels(&self) -> usize {
        match self {
            Self::Frame(j) => j.image.pixels(),
            _ => 0,
        }
    }
}
#[derive(Default)]
struct Queue {
    events: VecDeque<Work>,
    pixels: usize,
    closed: bool,
}
impl Queue {
    fn remove(&mut self, index: usize) -> Work {
        let work = self.events.remove(index).unwrap();
        self.pixels -= work.pixels();
        work
    }
    fn push(&mut self, work: Work) -> Result<bool> {
        if self.closed {
            bail!("encoder closed")
        }
        if let Work::Frame(job) = &work {
            if let Some(at) = self.events.iter().position(
                |old| matches!(old, Work::Frame(j) if j.window.window_id == job.window.window_id),
            ) {
                self.remove(at);
            }
            if work.pixels() > 8_000_000 {
                bail!("encoder pixel budget")
            }
            if self.pixels + work.pixels() > 8_000_000 {
                return Ok(false);
            }
        }
        if self.events.len() >= 128 {
            bail!("encoder queue full")
        }
        self.pixels += work.pixels();
        self.events.push_back(work);
        Ok(true)
    }
}
type WorkerQueue = Arc<(Mutex<Queue>, Condvar)>;
pub(super) struct Pool {
    queues: Vec<WorkerQueue>,
    output: Shared,
}
impl Pool {
    pub fn new(output: Shared, factory: Factory) -> Self {
        let queues: Vec<WorkerQueue> = (0..2)
            .map(|_| Arc::new((Mutex::new(Queue::default()), Condvar::new())))
            .collect();
        for shared in &queues {
            let shared = shared.clone();
            let output = output.clone();
            let factory = factory.clone();
            thread::spawn(move || {
                // Native GStreamer pointers never cross threads: create, use
                // and destroy every encoder exclusively inside this worker.
                let mut codec = factory();
                loop {
                    let work = {
                        let mut q = shared.0.lock().unwrap();
                        while q.events.is_empty() && !q.closed {
                            q = shared.1.wait(q).unwrap();
                        }
                        if q.closed {
                            break;
                        }
                        q.remove(0)
                    };
                    match work {
                        Work::Reset => codec.reset(),
                        Work::Forget(id) => codec.forget(id),
                        Work::Frame(job) => {
                            let result = codec.encode(&job);
                            let event = match result {
                                Ok(Some(value)) => Pending::Encoded(Encoded {
                                    window: job.window.window_id,
                                    generation: job.generation,
                                    value,
                                    session: job.session,
                                }),
                                Ok(None) => continue,
                                Err(_) => Pending::EncoderFailed {
                                    window: job.window.window_id,
                                    generation: job.generation,
                                },
                            };
                            let mut q = output.0.lock().unwrap();
                            if q.closed {
                                break;
                            }
                            if q.push(event).is_err() {
                                q.closed = true;
                            }
                            output.1.notify_one();
                        }
                    }
                }
            });
        }
        Self { queues, output }
    }
    fn push(&self, index: usize, work: Work) -> Result<()> {
        let id = match &work {
            Work::Frame(job) => Some(job.window.window_id),
            _ => None,
        };
        let q = &self.queues[index];
        if !q.0.lock().unwrap().push(work)? {
            // A dropped raw snapshot needs a fresh capture, not a keyframe
            // request from a client that may never have received any image.
            if let Some(id) = id {
                self.output.0.lock().unwrap().request_refresh(id)?;
            }
        }
        q.1.notify_one();
        Ok(())
    }
    pub fn frame(&self, job: Job) -> Result<()> {
        self.push(
            job.window.window_id as usize % self.queues.len(),
            Work::Frame(job),
        )
    }
    pub fn forget(&self, id: u32) -> Result<()> {
        self.push(id as usize % self.queues.len(), Work::Forget(id))
    }
    pub fn reset(&self) -> Result<()> {
        for index in 0..self.queues.len() {
            self.push(index, Work::Reset)?;
        }
        Ok(())
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        for q in &self.queues {
            let mut state = q.0.lock().unwrap();
            state.closed = true;
            state.events.clear();
            state.pixels = 0;
            q.1.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires Linux GStreamer x264/OpenH264"]
    fn gpu_failure_after_publication_restarts_software_with_new_stream_and_idr() {
        let mut job = job(1);
        job.window.width = 128;
        job.window.height = 64;
        job.image = Arc::new(crate::video::Frame::from(
            paneferry_backend_x11::CapturedImage {
                width: 128,
                height: 64,
                data: vec![96; 128 * 64 * 4],
                bpp: 32,
                pad: 32,
                little: true,
                masks: (0xff0000, 0xff00, 0xff),
            },
        ));
        let mut codec = RuntimeCodec {
            streams: Arc::new(AtomicU64::new(1)),
            windows: HashMap::new(),
            gpu_failed: false,
        };
        let first = codec.encode(&job).unwrap().unwrap();
        let state = codec.windows.get_mut(&1).unwrap();
        state.image = None;
        state.encoder.reject_gpu_input_for_test();
        let next = codec.encode(&job).unwrap().unwrap();
        assert_ne!(first["stream_id"], next["stream_id"]);
        assert_eq!(next["key_frame"], true);
        assert!(!codec.windows[&1].encoder.uses_gpu());
    }
    use paneferry_backend_x11::CapturedImage;

    fn job(id: u32) -> Job {
        Job {
            window: Window {
                window_id: id,
                parent_id: None,
                title: String::new(),
                presentation: None,
                role: "toplevel".into(),
                x: 0,
                y: 0,
                width: 3000,
                height: 2000,
                scale_factor: 1,
                geometry_revision: 1,
                visible: true,
            },
            // Queue accounting does not inspect pixels or call an encoder.
            image: Arc::new(crate::video::Frame::from(CapturedImage {
                width: 3000,
                height: 2000,
                data: vec![],
                bpp: 32,
                pad: 32,
                little: true,
                masks: (0xff0000, 0xff00, 0xff),
            })),
            session: "test".into(),
            generation: 1,
            fps: 60,
        }
    }

    #[test]
    fn pixel_pressure_preserves_older_windows_and_requests_static_recapture() {
        let output = Arc::new((Mutex::new(super::super::Queue::default()), Condvar::new()));
        let pool = Pool {
            queues: vec![Arc::new((Mutex::new(Queue::default()), Condvar::new()))],
            output: output.clone(),
        };
        pool.frame(job(1)).unwrap();
        pool.frame(job(2)).unwrap();
        assert_eq!(pool.queues[0].0.lock().unwrap().pixels, 6_000_000);
        assert!(
            matches!(pool.queues[0].0.lock().unwrap().remove(0), Work::Frame(j) if j.window.window_id == 1)
        );
        assert_eq!(
            output.0.lock().unwrap().refresh.drain().collect::<Vec<_>>(),
            vec![2]
        );
        // The recaptured static window obtains its turn when capacity is free.
        pool.frame(job(2)).unwrap();
        assert!(
            matches!(pool.queues[0].0.lock().unwrap().remove(0), Work::Frame(j) if j.window.window_id == 2)
        );
        assert!(output.0.lock().unwrap().refresh.is_empty());
    }
}
