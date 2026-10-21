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
    Message(Value, String),
    Frame(Window, CapturedImage, String),
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
        self.push(Pending::Frame(window.clone(), image, session.into()))
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
    let mut frames: HashMap<u32, (u64, CapturedImage)> = HashMap::new();
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
            Pending::Message(value, session) => {
                match value["type"].as_str() {
                    Some("session.snapshot" | "session.exited") => frames.clear(),
                    Some("window.destroyed") => {
                        frames.remove(&(value["window_id"].as_u64().unwrap_or(0) as u32));
                    }
                    Some("window.metadata") if value["visible"] == false => {
                        frames.remove(&(value["window_id"].as_u64().unwrap_or(0) as u32));
                    }
                    _ => {}
                }
                (value, session)
            }
            Pending::Frame(window, image, session) => {
                if frames
                    .get(&window.window_id)
                    .is_some_and(|(revision, old)| {
                        *revision == window.geometry_revision && *old == image
                    })
                {
                    continue;
                }
                let png = image.png()?;
                let value = json!({"type":"frame", "window_id":window.window_id, "geometry_revision":window.geometry_revision,
                    "width":window.width, "height":window.height, "png":STANDARD.encode(png)});
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
            CapturedImage {
                width: 1,
                height: 1,
                data: vec![0; 4],
                bpp: 32,
                pad: 32,
                little: true,
                masks: (0xff0000, 0xff00, 0xff),
            },
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
