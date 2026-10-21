//! Bounded output worker: PNG encoding and a slow reader must not stall X11 input.
use anyhow::{bail, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use remoteapp_backend_x11::CapturedImage;
use remoteapp_protocol::Window;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    io::Write,
    os::unix::net::UnixStream,
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
};

const PIXEL_BUDGET: usize = 16_000_000;
const EVENT_LIMIT: usize = 256;
enum Pending {
    EnableVideo,
    Message(Value, String),
    Frame(Window, Arc<CapturedImage>, String),
}
impl Pending {
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
    closed: bool,
}
impl Queue {
    fn remove(&mut self, index: usize) -> Pending {
        let event = self.events.remove(index).unwrap();
        self.pixels -= event.pixels();
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
            while self.pixels + event.pixels() > PIXEL_BUDGET {
                let Some(index) = self.events.iter().position(|old| old.frame_id().is_some())
                else {
                    bail!("frame exceeds output budget")
                };
                self.remove(index);
            }
        }
        if self.events.len() >= EVENT_LIMIT {
            bail!("output queue full")
        }
        self.pixels += event.pixels();
        self.events.push_back(event);
        Ok(())
    }
}
type Shared = Arc<(Mutex<Queue>, Condvar)>;
pub struct Output {
    queue: Shared,
    socket: UnixStream,
}
impl Output {
    pub fn new(mut socket: UnixStream) -> Result<Self> {
        socket.set_write_timeout(Some(Duration::from_secs(3)))?;
        let output = Self {
            queue: Arc::new((Mutex::new(Queue::default()), Condvar::new())),
            socket: socket.try_clone()?,
        };
        let shared = output.queue.clone();
        thread::spawn(move || {
            let _ = write_loop(&mut socket, &shared);
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
    pub fn frame(&self, window: &Window, image: CapturedImage, session: &str) -> Result<()> {
        self.push(Pending::Frame(
            window.clone(),
            Arc::new(image),
            session.into(),
        ))
    }
    pub fn enable_video(&self) -> Result<()> {
        self.push(Pending::EnableVideo)
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
fn write_loop(socket: &mut UnixStream, shared: &Shared) -> Result<()> {
    let mut sequence = 0u64;
    let mut frames: HashMap<u32, (u64, Arc<CapturedImage>)> = HashMap::new();
    let mut video = false;
    let mut encoders: HashMap<u32, crate::video::Encoder> = HashMap::new();
    let mut next_stream = 1;
    loop {
        let event = {
            let mut queue = shared.0.lock().unwrap();
            while !queue.closed && queue.events.is_empty() {
                queue = shared.1.wait(queue).unwrap();
            }
            if queue.closed {
                return Ok(());
            }
            queue.remove(0)
        };
        let (mut value, session) = match event {
            Pending::EnableVideo => {
                video = true;
                frames.clear();
                encoders.clear();
                continue;
            }
            Pending::Message(value, session) => {
                match value["type"].as_str() {
                    Some("session.snapshot" | "session.exited") => {
                        frames.clear();
                        encoders.clear();
                    }
                    Some("window.destroyed") => {
                        let id = value["window_id"].as_u64().unwrap_or(0) as u32;
                        frames.remove(&id);
                        encoders.remove(&id);
                    }
                    Some("window.metadata") if value["visible"] == false => {
                        let id = value["window_id"].as_u64().unwrap_or(0) as u32;
                        frames.remove(&id);
                        encoders.remove(&id);
                    }
                    _ => {}
                }
                (value, session)
            }
            Pending::Frame(window, image, session) => {
                if frames
                    .get(&window.window_id)
                    .is_some_and(|(revision, old)| {
                        *revision == window.geometry_revision && **old == *image
                    })
                {
                    continue;
                }
                let mut value = json!({"type":"frame", "window_id":window.window_id, "geometry_revision":window.geometry_revision,
                    "width":window.width, "height":window.height});
                let packet = if video {
                    let encoded = (|| -> Result<_> {
                        if encoders.get(&window.window_id).is_none_or(|encoder| {
                            encoder.width != image.width || encoder.height != image.height
                        }) {
                            encoders.remove(&window.window_id);
                            let encoder =
                                crate::video::Encoder::new(image.width, image.height, next_stream)?;
                            next_stream += 1;
                            encoders.insert(window.window_id, encoder);
                        }
                        let encoder = encoders.get_mut(&window.window_id).unwrap();
                        let packet = encoder.encode(&image)?;
                        Ok((packet, encoder.stream))
                    })();
                    match encoded {
                        Ok(packet) => Some(packet),
                        Err(_) => {
                            video = false;
                            encoders.clear();
                            value["video_error"] = json!("encoder_failed");
                            None
                        }
                    }
                } else {
                    None
                };
                if let Some((packet, stream)) = packet {
                    value["codec"] = json!("h264");
                    value["data"] = json!(STANDARD.encode(packet.bytes));
                    value["stream_id"] = json!(stream);
                    value["key_frame"] = json!(packet.keyframe);
                    value["encoded_width"] = json!(image.width + image.width % 2);
                    value["encoded_height"] = json!(image.height + image.height % 2);
                } else {
                    value["png"] = json!(STANDARD.encode(image.png()?));
                }
                frames.remove(&window.window_id);
                if frames
                    .values()
                    .map(|(_, image)| image.pixels())
                    .sum::<usize>()
                    + image.pixels()
                    > PIXEL_BUDGET
                {
                    frames.clear();
                }
                frames.insert(window.window_id, (window.geometry_revision, image));
                (value, session)
            }
        };
        sequence += 1;
        value["version"] = json!(1);
        value["seq"] = json!(sequence);
        value["session_id"] = json!(session);
        let mut bytes = serde_json::to_vec(&value)?;
        if bytes.len() + 1 > remoteapp_protocol::MAX_LINE {
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
            Arc::new(CapturedImage {
                width: 1,
                height: 1,
                data: vec![0; 4],
                bpp: 32,
                pad: 32,
                little: true,
                masks: (0xff0000, 0xff00, 0xff),
            }),
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
}
