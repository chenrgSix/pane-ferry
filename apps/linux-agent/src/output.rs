//! Bounded H.264 output worker: a slow reader must not stall input.
use crate::video_qos::VideoQos;
use anyhow::{bail, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
#[cfg(test)]
use paneferry_backend_x11::CapturedImage;
use paneferry_protocol::Window;
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
    EnableWebRtc(u8, bool),
    WebRtcAnswer(String),
    WebRtcCandidate(String, String),
    WebRtcEvent(u64, crate::webrtc::Event),
    VideoFallback,
    MediaPulse,
    ConfigureVideo(u8),
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
        self.push(Pending::Frame(window.clone(), image, session.into()))
    }
    pub fn take_refreshes(&self) -> Vec<u32> {
        self.queue.0.lock().unwrap().refresh.drain().collect()
    }
    pub fn enable_video(&self, max_fps: u8, feedback: bool) -> Result<()> {
        self.push(Pending::EnableVideo(max_fps, feedback))
    }
    pub fn enable_webrtc(&self, max_fps: u8, feedback: bool) -> Result<()> {
        if !crate::webrtc::available() {
            bail!("WebRTC runtime unavailable")
        }
        self.push(Pending::EnableWebRtc(max_fps, feedback))
    }
    pub fn webrtc_answer(&self, sdp: String) -> Result<()> {
        self.push(Pending::WebRtcAnswer(sdp))
    }
    pub fn webrtc_candidate(&self, candidate: String, mid: String) -> Result<()> {
        self.push(Pending::WebRtcCandidate(candidate, mid))
    }
    pub fn fallback_video(&self) -> Result<()> {
        self.push(Pending::VideoFallback)
    }
    pub fn configure_video(&self, max_fps: u8) -> Result<()> {
        self.push(Pending::ConfigureVideo(max_fps))
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
    let mut rtc: Option<crate::webrtc::Peer> = None;
    let mut rtc_epoch = 0u64;
    let mut media_frames: HashMap<u32, u64> = HashMap::new();
    let mut last_pulse = Instant::now();
    loop {
        let event = {
            let mut queue = shared.0.lock().unwrap();
            loop {
                if queue.closed {
                    return Ok(());
                }
                if rtc.is_some() && last_pulse.elapsed() >= Duration::from_secs(1) {
                    break Pending::MediaPulse;
                }
                if !queue.events.is_empty() {
                    break queue.remove(0);
                }
                if rtc.is_some() {
                    let delay = Duration::from_secs(1).saturating_sub(last_pulse.elapsed());
                    let (next, timed) = shared.1.wait_timeout(queue, delay).unwrap();
                    queue = next;
                    if timed.timed_out() {
                        break Pending::MediaPulse;
                    }
                } else {
                    queue = shared.1.wait(queue).unwrap();
                }
            }
        };
        let (mut value, session) = match event {
            Pending::EnableVideo(max_fps, feedback) => {
                rtc = None;
                rtc_epoch = rtc_epoch.wrapping_add(1);
                media_frames.clear();
                video = true;
                generations.clear();
                geometry.clear();
                workers.reset()?;
                qos.lock()
                    .unwrap()
                    .configure(max_fps, feedback, Instant::now());
                continue;
            }
            Pending::EnableWebRtc(max_fps, feedback) => {
                rtc_epoch = rtc_epoch.wrapping_add(1);
                let epoch = rtc_epoch;
                let notifications = shared.clone();
                rtc = Some(crate::webrtc::Peer::new(move |event| {
                    let mut queue = notifications.0.lock().unwrap();
                    if queue.push(Pending::WebRtcEvent(epoch, event)).is_err() {
                        queue.closed = true;
                    }
                    drop(queue);
                    notifications.1.notify_one();
                })?);
                video = true;
                media_frames.clear();
                generations.clear();
                geometry.clear();
                workers.reset()?;
                qos.lock()
                    .unwrap()
                    .configure(max_fps, feedback, Instant::now());
                continue;
            }
            Pending::WebRtcAnswer(sdp) => {
                if let Some(peer) = &rtc {
                    peer.answer(&sdp)?;
                }
                continue;
            }
            Pending::WebRtcCandidate(candidate, mid) => {
                if let Some(peer) = &rtc {
                    peer.candidate(&candidate, &mid)?;
                }
                continue;
            }
            Pending::WebRtcEvent(epoch, event) => {
                if epoch != rtc_epoch || rtc.is_none() {
                    continue;
                }
                use crate::webrtc::Event;
                match event {
                    Event::Description(sdp) => {
                        (json!({"type":"webrtc.offer","sdp":sdp}), String::new())
                    }
                    Event::Candidate { candidate, mid } => (
                        json!({"type":"webrtc.candidate","candidate":candidate,"mid":mid}),
                        String::new(),
                    ),
                    Event::Keyframe(id) => {
                        if geometry.contains_key(&id) {
                            next_generation += 1;
                            generations.insert(id, next_generation);
                            workers.forget(id)?;
                            shared.0.lock().unwrap().request_refresh(id)?;
                        }
                        continue;
                    }
                    Event::State(state) if !matches!(state, 4 | 5) => {
                        let pair = (state == 2)
                            .then(|| rtc.as_ref().and_then(|p| p.selected_pair()))
                            .flatten();
                        (
                            json!({"type":"webrtc.state","state":state,"pair":pair}),
                            String::new(),
                        )
                    }
                    Event::State(_) | Event::Failed => {
                        retire_media(
                            &mut rtc,
                            &mut rtc_epoch,
                            &mut media_frames,
                            &mut generations,
                            &workers,
                            shared,
                            qos,
                        )?;
                        (
                            json!({"type":"webrtc.failed","code":"media_transport_failed"}),
                            String::new(),
                        )
                    }
                }
            }
            Pending::VideoFallback => {
                if rtc.is_some() {
                    retire_media(
                        &mut rtc,
                        &mut rtc_epoch,
                        &mut media_frames,
                        &mut generations,
                        &workers,
                        shared,
                        qos,
                    )?;
                }
                continue;
            }
            Pending::MediaPulse => {
                last_pulse = Instant::now();
                if rtc.is_none() {
                    continue;
                }
                (
                    json!({"type":"webrtc.pulse","frames":media_frames}),
                    String::new(),
                )
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
            Pending::KeyFrame(id) => {
                next_generation += 1;
                generations.insert(id, next_generation);
                workers.forget(id)?;
                continue;
            }
            Pending::Message(value, session) => {
                match value["type"].as_str() {
                    Some("session.snapshot" | "session.exited") => {
                        media_frames.clear();
                        generations.clear();
                        geometry.clear();
                        workers.reset()?;
                    }
                    Some("window.destroyed") => {
                        let id = value["window_id"].as_u64().unwrap_or(0) as u32;
                        generations.remove(&id);
                        media_frames.remove(&id);
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
                if !video {
                    bail!("H.264 must be enabled before attaching")
                }
                if !generations.contains_key(&window.window_id) {
                    next_generation += 1;
                    generations.insert(window.window_id, next_generation);
                }
                if let Some(peer) = &rtc {
                    match peer.ready(window.window_id) {
                        Ok(true) => {}
                        Ok(false) => {
                            shared.0.lock().unwrap().request_refresh(window.window_id)?;
                            continue;
                        }
                        Err(_) => {
                            retire_media(
                                &mut rtc,
                                &mut rtc_epoch,
                                &mut media_frames,
                                &mut generations,
                                &workers,
                                shared,
                                qos,
                            )?;
                            shared.0.lock().unwrap().push(Pending::Message(
                                json!({"type":"webrtc.failed","code":"track_failed"}),
                                session,
                            ))?;
                            continue;
                        }
                    }
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
                if generations.get(&encoded.window) != Some(&encoded.generation) {
                    continue;
                }
                if let Some(id) = qos.lock().unwrap().sent(Instant::now()) {
                    encoded.value["frame_id"] = json!(id);
                }
                if let Some(peer) = &rtc {
                    let media_id = media_frames.get(&encoded.window).copied().unwrap_or(0) + 1;
                    encoded.value["media_frame_id"] = json!(media_id);
                    if encoded.value.get("frame_id").is_none() {
                        encoded.value["frame_id"] = json!(media_id);
                    }
                    encoded.value["transport"] = json!("webrtc");
                    encoded.value["version"] = json!(1);
                    encoded.value["session_id"] = json!(encoded.session);
                    encoded.value["seq"] = json!(sequence + 1);
                    let data = encoded.value.as_object_mut().unwrap().remove("data");
                    let Some(Value::String(data)) = data else {
                        bail!("missing encoded frame")
                    };
                    let bytes = STANDARD.decode(data)?;
                    let metadata = serde_json::to_vec(&encoded.value)?;
                    if peer.send(encoded.window, &bytes, &metadata).is_err() {
                        retire_media(
                            &mut rtc,
                            &mut rtc_epoch,
                            &mut media_frames,
                            &mut generations,
                            &workers,
                            shared,
                            qos,
                        )?;
                        encoded.value = json!({"type":"webrtc.failed","code":"media_send_failed"});
                    } else {
                        media_frames.insert(encoded.window, media_id);
                        sequence += 1;
                        continue;
                    }
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
        if rtc.is_some() {
            // QUIC assigns a delivery sequence after merging its streams.
            // Direct RTP retains this source identity for lifecycle barriers.
            value["source_seq"] = json!(sequence);
        }
        let mut bytes = serde_json::to_vec(&value)?;
        if bytes.len() + 1 > paneferry_protocol::MAX_LINE {
            bail!("outgoing frame too large")
        }
        bytes.push(b'\n');
        socket.write_all(&bytes)?;
    }
}

// Retire only video generations. The authenticated QUIC control connection,
// controller lease, application and negotiated frame-rate cap stay alive.
fn retire_media(
    rtc: &mut Option<crate::webrtc::Peer>,
    epoch: &mut u64,
    frames: &mut HashMap<u32, u64>,
    generations: &mut HashMap<u32, u64>,
    workers: &worker::Pool,
    shared: &Shared,
    qos: &Arc<Mutex<VideoQos>>,
) -> Result<()> {
    *rtc = None;
    *epoch = epoch.wrapping_add(1);
    frames.clear();
    workers.reset()?;
    let mut queue = shared.0.lock().unwrap();
    // A resumed static window is described by session.snapshot rather than
    // window.created. Its active encoder generation must also be recaptured.
    for id in generations.keys() {
        queue.request_refresh(*id)?;
    }
    generations.clear();
    drop(queue);
    let mut state = qos.lock().unwrap();
    let cap = state.cap();
    let feedback = state.feedback_enabled();
    state.configure(cap, feedback, Instant::now());
    Ok(())
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
