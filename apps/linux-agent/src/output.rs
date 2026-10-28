//! Bounded H.264 output worker: a slow reader must not stall input.
use crate::video_qos::VideoQos;
use anyhow::{bail, Result};
#[cfg(test)]
use paneferry_backend_x11::CapturedImage;
use paneferry_protocol::{VideoActivity, Window};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::Write,
    os::unix::net::UnixStream,
    sync::{Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};

#[path = "output_worker.rs"]
mod worker;

const PIXEL_BUDGET: usize = 16_000_000;
const EVENT_LIMIT: usize = 256;
enum Pending {
    EnableVideo(u8, bool),
    ConfigureVideo(u8),
    Activity(VideoActivity),
    KeyFrame(u32),
    Message(Value, String),
    Frame(Window, Arc<crate::video::Frame>, String),
    Encoded(worker::Encoded),
    EncoderFailed { window: u32, generation: u64 },
}
impl Pending {
    fn encoded_bytes(&self) -> usize {
        match self {
            Self::Encoded(packet) => packet.value["data"].as_str().map_or(0, str::len),
            _ => 0,
        }
    }
    fn pixels(&self) -> usize {
        match self {
            Self::Frame(_, image, _) => image.pixels(),
            _ => 0,
        }
    }
    fn frame_id(&self) -> Option<u32> {
        match self {
            Self::Frame(window, _, _) => Some(window.window_id),
            _ => None,
        }
    }
}
#[derive(Default)]
struct Queue {
    events: VecDeque<Pending>,
    pixels: usize,
    encoded_bytes: usize,
    refresh: HashSet<u32>,
    closed: bool,
}
impl Queue {
    fn request_refresh(&mut self, id: u32) -> Result<()> {
        if self.refresh.len() >= 64 && !self.refresh.contains(&id) {
            bail!("recapture window limit")
        }
        self.refresh.insert(id);
        Ok(())
    }
    fn remove(&mut self, index: usize) -> Pending {
        let event = self.events.remove(index).unwrap();
        self.pixels -= event.pixels();
        self.encoded_bytes -= event.encoded_bytes();
        event
    }
    fn push(&mut self, event: Pending) -> Result<()> {
        if self.closed {
            bail!("output closed")
        }
        if let Some(id) = event.frame_id() {
            // Append the newest frame after intervening metadata/control events.
            if let Some(index) = self
                .events
                .iter()
                .position(|old| old.frame_id() == Some(id))
            {
                self.remove(index);
            }
            if event.pixels() > PIXEL_BUDGET {
                bail!("frame exceeds output budget")
            }
            if self.pixels + event.pixels() > PIXEL_BUDGET {
                // Capture already acknowledged XDamage. Preserve older windows'
                // turns and explicitly request this window again, even if static.
                self.request_refresh(id)?;
                return Ok(());
            }
        }
        if self.encoded_bytes + event.encoded_bytes() > 32 * 1024 * 1024 {
            bail!("encoded output budget exceeded")
        }
        if self.events.len() >= EVENT_LIMIT {
            bail!("output queue full")
        }
        self.pixels += event.pixels();
        self.encoded_bytes += event.encoded_bytes();
        self.events.push_back(event);
        Ok(())
    }
}
type Shared = Arc<(Mutex<Queue>, Condvar)>;
#[derive(Clone)]
pub struct VideoFeedback(Arc<Mutex<VideoQos>>);
impl VideoFeedback {
    pub fn acknowledge(&self, frame_id: u64) {
        self.0.lock().unwrap().acknowledge(frame_id, Instant::now());
    }
}
pub struct Output {
    queue: Shared,
    socket: UnixStream,
    qos: Arc<Mutex<VideoQos>>,
}
impl Output {
    pub fn new(socket: UnixStream) -> Result<Self> {
        Self::with_factory(socket, worker::factory())
    }
    fn with_factory(mut socket: UnixStream, factory: worker::Factory) -> Result<Self> {
        socket.set_write_timeout(Some(Duration::from_secs(3)))?;
        let qos = Arc::new(Mutex::new(VideoQos::new(Instant::now())));
        let output = Self {
            queue: Arc::new((Mutex::new(Queue::default()), Condvar::new())),
            socket: socket.try_clone()?,
            qos: qos.clone(),
        };
        let shared = output.queue.clone();
        thread::spawn(move || {
            let _ = write_loop(&mut socket, &shared, &qos, factory);
            let mut queue = shared.0.lock().unwrap();
            queue.closed = true;
            queue.events.clear();
            queue.pixels = 0;
            // Wake the request reader too, releasing held keys/controller leases.
            let _ = socket.shutdown(std::net::Shutdown::Both);
        });
        Ok(output)
    }
    fn push(&self, event: Pending) -> Result<()> {
        self.queue.0.lock().unwrap().push(event)?;
        self.queue.1.notify_one();
        Ok(())
    }
    pub fn message(&self, value: Value, session: &str) -> Result<()> {
        self.push(Pending::Message(value, session.into()))
    }
    pub fn frame(
        &self,
        window: &Window,
        image: Arc<crate::video::Frame>,
        session: &str,
    ) -> Result<()> {
        if self.qos.lock().unwrap().suspended() {
            return Ok(());
        }
        self.push(Pending::Frame(window.clone(), image, session.into()))
    }
    pub fn take_refreshes(&self) -> Vec<u32> {
        self.queue.0.lock().unwrap().refresh.drain().collect()
    }
    pub fn enable_video(&self, max_fps: u8, feedback: bool) -> Result<()> {
        self.push(Pending::EnableVideo(max_fps, feedback))
    }
    pub fn configure_video(&self, max_fps: u8) -> Result<()> {
        self.push(Pending::ConfigureVideo(max_fps))
    }
    pub fn activity(&self, state: VideoActivity) -> Result<()> {
        self.qos.lock().unwrap().set_activity(state, Instant::now());
        self.push(Pending::Activity(state))
    }
    pub fn keyframe(&self, id: u32) -> Result<()> {
        self.push(Pending::KeyFrame(id))
    }
    pub fn feedback_handle(&self) -> VideoFeedback {
        VideoFeedback(self.qos.clone())
    }
    pub fn video_feedback(&self, frame_id: u64) {
        self.feedback_handle().acknowledge(frame_id);
    }
    pub fn frame_interval(&self) -> Duration {
        self.qos.lock().unwrap().interval(Instant::now())
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        let mut queue = self.queue.0.lock().unwrap();
        queue.closed = true;
        queue.events.clear();
        queue.pixels = 0;
        self.queue.1.notify_one();
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }
}
fn write_loop(
    socket: &mut UnixStream,
    shared: &Shared,
    qos: &Arc<Mutex<VideoQos>>,
    factory: worker::Factory,
) -> Result<()> {
    let workers = worker::Pool::new(shared.clone(), factory);
    let mut sequence = 0u64;
    let mut next_generation = 0u64;
    let mut generations: HashMap<u32, u64> = HashMap::new();
    let mut geometry: HashMap<u32, Value> = HashMap::new();
    let mut video = false;
    let mut suspended = false;
    loop {
        let event = {
            let mut queue = shared.0.lock().unwrap();
            loop {
                if queue.closed {
                    return Ok(());
                }
                if !queue.events.is_empty() {
                    break queue.remove(0);
                }
                queue = shared.1.wait(queue).unwrap();
            }
        };
        let (mut value, session) = match event {
            Pending::EnableVideo(max_fps, feedback) => {
                video = true;
                generations.clear();
                geometry.clear();
                workers.reset()?;
                qos.lock()
                    .unwrap()
                    .configure(max_fps, feedback, Instant::now());
                continue;
            }
            Pending::ConfigureVideo(max_fps) => {
                if !video {
                    bail!("video not enabled")
                }
                generations.clear();
                geometry.clear();
                workers.reset()?;
                let mut state = qos.lock().unwrap();
                let feedback = state.feedback_enabled();
                state.configure(max_fps, feedback, Instant::now());
                continue;
            }
            Pending::Activity(state) => {
                if !video {
                    bail!("video not enabled")
                }
                let pause = state == VideoActivity::Suspended;
                if pause != suspended {
                    generations.clear();
                    geometry.clear();
                    workers.reset()?;
                }
                suspended = pause;
                // Acknowledgement follows all already written video. Once it
                // reports suspension, no queued/in-flight frame may follow.
                (
                    json!({"type":"video.activity.changed", "state":state}),
                    String::new(),
                )
            }
            Pending::KeyFrame(id) => {
                next_generation += 1;
                generations.insert(id, next_generation);
                workers.forget(id)?;
                continue;
            }
            Pending::Message(value, session) => {
                match value["type"].as_str() {
                    Some("session.snapshot" | "session.exited") => {
                        generations.clear();
                        geometry.clear();
                        workers.reset()?;
                    }
                    Some("window.destroyed") => {
                        let id = value["window_id"].as_u64().unwrap_or(0) as u32;
                        generations.remove(&id);
                        geometry.remove(&id);
                        shared.0.lock().unwrap().refresh.remove(&id);
                        workers.forget(id)?;
                    }
                    Some("window.created" | "window.metadata") => {
                        let id = value["window_id"].as_u64().unwrap_or(0) as u32;
                        let shape = json!([
                            value["geometry_revision"],
                            value["width"],
                            value["height"],
                            value["visible"]
                        ]);
                        if value["type"] == "window.created" || geometry.get(&id) != Some(&shape) {
                            next_generation += 1;
                            generations.insert(id, next_generation);
                            workers.forget(id)?;
                        }
                        geometry.insert(id, shape);
                    }
                    _ => {}
                }
                (value, session)
            }
            Pending::Frame(window, image, session) => {
                if suspended {
                    continue;
                }
                if !video {
                    bail!("H.264 must be enabled before attaching")
                }
                if !generations.contains_key(&window.window_id) {
                    next_generation += 1;
                    generations.insert(window.window_id, next_generation);
                }
                if generations.len() > 64 {
                    bail!("video window limit")
                }
                let generation = generations[&window.window_id];
                workers.frame(worker::Job {
                    window,
                    image,
                    session,
                    generation,
                    fps: qos.lock().unwrap().cap(),
                })?;
                continue;
            }
            Pending::Encoded(mut encoded) => {
                // Metadata/destroy can overtake an encoder. A fresh generation
                // restarts with IDR; its predecessor can never appear afterward.
                if suspended || generations.get(&encoded.window) != Some(&encoded.generation) {
                    continue;
                }
                if let Some(id) = qos.lock().unwrap().sent(Instant::now()) {
                    encoded.value["frame_id"] = json!(id);
                }
                (encoded.value, encoded.session)
            }
            Pending::EncoderFailed { window, generation } => {
                if generations.get(&window) == Some(&generation) {
                    bail!("video encoder failed")
                }
                continue;
            }
        };
        sequence += 1;
        value["version"] = json!(1);
        value["seq"] = json!(sequence);
        value["session_id"] = json!(session);
        let mut bytes = serde_json::to_vec(&value)?;
        if bytes.len() + 1 > paneferry_protocol::MAX_LINE {
            bail!("outgoing frame too large")
        }
        bytes.push(b'\n');
        socket.write_all(&bytes)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(revision: u64) -> Pending {
        let window = Window {
            window_id: 7,
            parent_id: None,
            title: String::new(),
            presentation: None,
            role: "toplevel".into(),
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            scale_factor: 1,
            geometry_revision: revision,
            visible: true,
        };
        Pending::Frame(
            window,
            Arc::new(crate::video::Frame::from(CapturedImage {
                width: 1,
                height: 1,
                data: vec![0; 4],
                bpp: 32,
                pad: 32,
                little: true,
                masks: (0xff0000, 0xff00, 0xff),
            })),
            "session".into(),
        )
    }
    #[test]
    fn coalesces_frames_after_metadata_without_reordering_control_events() {
        let mut queue = Queue::default();
        queue.push(frame(1)).unwrap();
        queue
            .push(Pending::Message(
                json!({"type":"window.metadata"}),
                "session".into(),
            ))
            .unwrap();
        queue.push(frame(2)).unwrap();
        assert_eq!(queue.events.len(), 2);
        assert_eq!(queue.pixels, 1);
        assert!(matches!(queue.remove(0), Pending::Message(..)));
        assert!(matches!(
            queue.remove(0),
            Pending::Frame(
                Window {
                    geometry_revision: 2,
                    ..
                },
                ..
            )
        ));
        assert_eq!(queue.pixels, 0);
    }
    #[test]
    fn suspended_video_drops_inflight_output_keeps_control_live_and_resumes_a_fresh_generation() {
        use std::io::{BufRead, BufReader};
        struct Slow {
            first: bool,
            entered: std::sync::mpsc::Sender<()>,
            release: Arc<(Mutex<bool>, Condvar)>,
        }
        impl worker::Codec for Slow {
            fn encode(&mut self, job: &worker::Job) -> Result<Option<Value>> {
                if self.first {
                    self.first = false;
                    self.entered.send(()).unwrap();
                    let (lock, wake) = &*self.release;
                    let (ready, _) = wake
                        .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(3), |ready| {
                            !*ready
                        })
                        .unwrap();
                    assert!(*ready);
                }
                Ok(Some(
                    json!({"type":"frame", "data":"AA==", "generation":job.generation}),
                ))
            }
            fn forget(&mut self, _: u32) {}
            fn reset(&mut self) {}
        }
        let (entered, signal) = std::sync::mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let gate = release.clone();
        let factory: worker::Factory = Arc::new(move || {
            Box::new(Slow {
                first: true,
                entered: entered.clone(),
                release: gate.clone(),
            })
        });
        let (writer, socket) = UnixStream::pair().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let output = Output::with_factory(writer, factory).unwrap();
        let mut reader = BufReader::new(socket);
        let mut read = || {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        };
        output.enable_video(30, true).unwrap();
        output.push(frame(1)).unwrap();
        signal.recv_timeout(Duration::from_secs(2)).unwrap();
        output.activity(VideoActivity::Suspended).unwrap();
        assert_eq!(read()["state"], "suspended");
        output.push(frame(1)).unwrap();
        output
            .message(json!({"type":"fixture.input"}), "session")
            .unwrap();
        assert_eq!(
            read()["type"],
            "fixture.input",
            "paused video must not block input/metadata"
        );
        output.activity(VideoActivity::Foreground).unwrap();
        assert_eq!(read()["state"], "foreground");
        output.push(frame(1)).unwrap();
        *release.0.lock().unwrap() = true;
        release.1.notify_all();
        let packet = read();
        assert_eq!(packet["type"], "frame");
        assert!(
            packet["generation"].as_u64().unwrap() > 1,
            "the pre-suspend encoder result must be retired"
        );
    }
    #[test]
    fn output_pixel_pressure_retains_other_windows_and_requests_recapture() {
        let mut queue = Queue::default();
        for id in 1..=3 {
            let Pending::Frame(mut window, _, session) = frame(1) else {
                unreachable!()
            };
            window.window_id = id;
            let image = Arc::new(crate::video::Frame::from(CapturedImage {
                width: 4000,
                height: 2000,
                data: vec![],
                bpp: 32,
                pad: 32,
                little: true,
                masks: (0xff0000, 0xff00, 0xff),
            }));
            queue.push(Pending::Frame(window, image, session)).unwrap();
        }
        assert_eq!(queue.pixels, PIXEL_BUDGET);
        assert_eq!(queue.events.len(), 2);
        assert_eq!(queue.remove(0).frame_id(), Some(1));
        assert_eq!(queue.remove(0).frame_id(), Some(2));
        assert_eq!(queue.refresh.into_iter().collect::<Vec<_>>(), vec![3]);
    }

    #[test]
    fn bounds_control_queue_and_rejects_writes_after_close() {
        let mut queue = Queue::default();
        for _ in 0..EVENT_LIMIT {
            queue
                .push(Pending::Message(json!({}), String::new()))
                .unwrap();
        }
        assert!(queue.push(frame(1)).is_err());
        queue.closed = true;
        assert!(queue
            .push(Pending::Message(json!({}), String::new()))
            .is_err());
    }

    #[test]
    fn blocked_encoding_does_not_hold_control_or_other_worker_and_drops_retired_result() {
        use std::{
            io::{BufRead, BufReader},
            sync::mpsc,
        };
        struct Slow {
            entered: mpsc::Sender<()>,
            release: Arc<(Mutex<bool>, Condvar)>,
            first: bool,
        }
        impl worker::Codec for Slow {
            fn encode(&mut self, job: &worker::Job) -> Result<Option<Value>> {
                if job.window.window_id == 7 && self.first {
                    self.first = false;
                    self.entered.send(()).unwrap();
                    let (lock, ready) = &*self.release;
                    let released = lock.lock().unwrap();
                    let (released, _) = ready
                        .wait_timeout_while(released, Duration::from_secs(3), |v| !*v)
                        .unwrap();
                    assert!(*released, "test did not release encoder");
                }
                Ok(Some(
                    json!({"type":"frame", "window_id":job.window.window_id, "data":"AA==", "generation":job.generation}),
                ))
            }
            fn forget(&mut self, _: u32) {}
            fn reset(&mut self) {}
        }
        let (entered, signal) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let gate = release.clone();
        let factory: worker::Factory = Arc::new(move || {
            Box::new(Slow {
                entered: entered.clone(),
                release: gate.clone(),
                first: true,
            })
        });
        let (writer, reader) = UnixStream::pair().unwrap();
        reader
            .set_read_timeout(Some(Duration::from_millis(750)))
            .unwrap();
        let output = Output::with_factory(writer, factory).unwrap();
        let mut reader = BufReader::new(reader);
        let mut read = || {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        };
        let Pending::Frame(mut window, image, session) = frame(1) else {
            unreachable!()
        };
        output.enable_video(60, false).unwrap();
        output
            .message(json!({"type":"window.created", "window_id":7}), &session)
            .unwrap();
        assert_eq!(read()["type"], "window.created");
        output.frame(&window, image.clone(), &session).unwrap();
        signal.recv_timeout(Duration::from_secs(2)).unwrap();
        output
            .message(json!({"type":"clipboard.data", "text":"fixture"}), &session)
            .unwrap();
        assert_eq!(read()["type"], "clipboard.data");
        window.window_id = 6;
        output.frame(&window, image.clone(), &session).unwrap();
        assert_eq!(read()["window_id"], 6);
        output
            .message(json!({"type":"window.destroyed", "window_id":7}), &session)
            .unwrap();
        assert_eq!(read()["type"], "window.destroyed");
        output
            .message(json!({"type":"window.created", "window_id":7}), &session)
            .unwrap();
        assert_eq!(read()["type"], "window.created");
        window.window_id = 7;
        output.frame(&window, image, &session).unwrap();
        *release.0.lock().unwrap() = true;
        release.1.notify_all();
        let current = read();
        assert_eq!(current["window_id"], 7);
        assert!(
            current["generation"].as_u64().unwrap() > 1,
            "late encoder output must not revive the destroyed generation"
        );
    }

    #[test]
    #[ignore = "requires Linux GStreamer/OpenH264; run explicitly on Ubuntu"]
    fn h264_frames_keep_one_stream_without_png_fallback() {
        use std::io::{BufRead, BufReader};
        let (writer, reader) = UnixStream::pair().unwrap();
        reader
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        let output = Output::new(writer).unwrap();
        let mut reader = BufReader::new(reader);
        let mut read = || {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        };
        let Pending::Frame(mut window, _, session) = frame(1) else {
            unreachable!()
        };
        window.width = 128;
        window.height = 64;
        let image = CapturedImage {
            width: 128,
            height: 64,
            bpp: 32,
            pad: 32,
            little: true,
            masks: (0xff0000, 0xff00, 0xff),
            data: (0..128 * 64)
                .flat_map(|i| {
                    if i % 3 == 0 {
                        [0, 0, 255, 0]
                    } else {
                        [255, 255, 255, 0]
                    }
                })
                .collect(),
        };
        let mut changed = CapturedImage {
            data: image.data.clone(),
            ..image
        };
        output.enable_video(60, false).unwrap();
        output
            .frame(&window, Arc::new(image.into()), &session)
            .unwrap();
        let video = read();
        assert_eq!(video["codec"], "h264");
        assert!(video.get("png").is_none());
        changed.data[0] = 125;
        output
            .frame(&window, Arc::new(changed.into()), &session)
            .unwrap();
        let moving = read();
        assert_eq!(moving["codec"], "h264");
        assert_eq!(moving["stream_id"], video["stream_id"]);
        assert_eq!(moving["key_frame"], false);
        assert!(moving.get("png").is_none());
        output
            .message(json!({"type":"test.barrier"}), &session)
            .unwrap();
        assert_eq!(read()["type"], "test.barrier");
    }
}
