//! Capture COM objects never leave their owning thread. Control only shares a
//! small validated layout snapshot; it never locks across capture or encoding.
use super::*;
use crate::FrameTask;
use std::sync::{mpsc, Arc, Mutex};
use tokio::sync::oneshot;

type Snapshot = Arc<Mutex<(Geometry, scene::InputScene)>>;
enum Work {
    Frame(oneshot::Sender<Result<Option<VideoFrame>>>),
    Fps(u32),
    Quality(u32),
    Keyframe,
}
struct Worker {
    target: Target,
    tx: Option<mpsc::SyncSender<Work>>,
    snapshot: Snapshot,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        // Close commands and wait for COM destruction/lease release before a new
        // attach can claim the same application. At most one frame is in flight.
        self.tx.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Worker {
    fn send(&self, work: Work) -> Result<()> {
        self.tx
            .as_ref()
            .context("capture_failed")?
            .try_send(work)
            .map_err(|_| anyhow::anyhow!("capture_failed"))
    }
    fn new(target: &Target, fps: u32) -> Result<Self> {
        let (tx, rx) = mpsc::sync_channel(8);
        let (ready, initialized) = mpsc::sync_channel::<Result<Snapshot>>(1);
        let captured_target = target.clone();
        let thread = std::thread::Builder::new()
            .name("paneferry.capture".into())
            .spawn(move || {
                let initialize = || -> Result<(Desktop, Snapshot)> {
                    let mut desktop = Desktop::new()?;
                    desktop.attach(&captured_target, fps)?;
                    let geometry = desktop.geometry(&captured_target)?;
                    let snapshot = Arc::new(Mutex::new((
                        geometry,
                        desktop.scene.as_ref().unwrap().input_snapshot(),
                    )));
                    Ok((desktop, snapshot))
                };
                let (mut desktop, snapshot) = match initialize() {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                if ready.send(Ok(snapshot.clone())).is_err() {
                    return;
                }
                let mut fault: Option<anyhow::Error> = None;
                while let Ok(work) = rx.recv() {
                    match work {
                        Work::Fps(fps) => {
                            if let Err(error) = desktop.set_video_fps(fps) {
                                fault = Some(error);
                            }
                        }
                        Work::Quality(quality) => {
                            if let Err(error) = desktop.set_video_quality(quality) {
                                eprintln!("PF_ENCODER quality_update_failed={error:#}");
                            }
                        }
                        Work::Keyframe => {
                            if let Err(error) = desktop.keyframe(&captured_target) {
                                fault = Some(error);
                            }
                        }
                        Work::Frame(reply) => {
                            if reply.is_closed() {
                                continue;
                            }
                            let result = if let Some(error) = fault.take() {
                                Err(error)
                            } else {
                                let signal = desktop.scene.as_ref().unwrap().signal.clone();
                                let deadline = std::time::Instant::now()
                                    + std::time::Duration::from_secs_f64(
                                        1.0 / desktop.fps.max(30) as f64,
                                    );
                                loop {
                                    let generation = signal.generation();
                                    let result = desktop.frame(&captured_target);
                                    let scene = desktop.scene.as_ref().unwrap();
                                    *snapshot.lock().unwrap() =
                                        (scene.current_geometry(), scene.input_snapshot());
                                    if !matches!(&result, Ok(None)) || reply.is_closed() {
                                        break result;
                                    }
                                    let now = std::time::Instant::now();
                                    if now >= deadline {
                                        break result;
                                    }
                                    signal.wait(generation, deadline - now);
                                }
                            };
                            let _ = reply.send(result);
                        }
                    }
                }
            })?;
        let snapshot = initialized.recv().context("capture_failed")??;
        Ok(Self {
            target: target.clone(),
            tx: Some(tx),
            snapshot,
            thread: Some(thread),
        })
    }
}

pub struct PublishingDesktop {
    control: Desktop,
    worker: Option<Worker>,
}
impl PublishingDesktop {
    pub fn new() -> Result<Self> {
        Ok(Self {
            control: Desktop::new()?,
            worker: None,
        })
    }
}
impl WindowSystem for PublishingDesktop {
    fn applications(&mut self) -> Result<Vec<crate::Application>> {
        self.control.applications()
    }
    fn launch_application(&mut self, id: &str, start: bool) -> Result<Target> {
        self.control.launch_application(id, start)
    }
    fn windows(&mut self) -> Result<Vec<Target>> {
        self.control.windows()
    }
    fn geometry(&mut self, target: &Target) -> Result<Geometry> {
        if let Some(worker) = self.worker.as_ref().filter(|w| w.target.id == target.id) {
            validate(target)?;
            return Ok(worker.snapshot.lock().unwrap().0.clone());
        }
        self.control.geometry(target)
    }
    fn attach(&mut self, target: &Target, fps: u32) -> Result<()> {
        self.detach();
        self.worker = Some(Worker::new(target, fps)?);
        Ok(())
    }
    fn set_video_fps(&mut self, fps: u32) -> Result<()> {
        if let Some(worker) = &self.worker {
            worker.send(Work::Fps(fps))?;
        }
        Ok(())
    }
    fn set_video_quality(&mut self, quality: u32) -> Result<()> {
        if let Some(worker) = &self.worker {
            worker.send(Work::Quality(quality))?;
        }
        Ok(())
    }
    fn keyframe(&mut self, target: &Target) -> Result<()> {
        let worker = self
            .worker
            .as_ref()
            .filter(|w| w.target.id == target.id)
            .context("session_exited")?;
        worker.send(Work::Keyframe)
    }
    fn begin_frame(&mut self, target: &Target) -> Result<FrameTask> {
        let worker = self
            .worker
            .as_ref()
            .filter(|w| w.target.id == target.id)
            .context("session_exited")?;
        let (tx, rx) = oneshot::channel();
        worker.send(Work::Frame(tx))?;
        Ok(Box::pin(async move { rx.await.context("capture_failed")? }))
    }
    fn frame(&mut self, target: &Target) -> Result<Option<VideoFrame>> {
        let worker = self
            .worker
            .as_ref()
            .filter(|w| w.target.id == target.id)
            .context("session_exited")?;
        let (tx, rx) = oneshot::channel();
        worker.send(Work::Frame(tx))?;
        rx.blocking_recv().context("capture_failed")?
    }
    fn input(&mut self, target: &Target, input: Input) -> Result<()> {
        let worker = self
            .worker
            .as_ref()
            .filter(|w| w.target.id == target.id)
            .context("session_exited")?;
        self.control.input_scene = Some(worker.snapshot.lock().unwrap().1.clone());
        self.control.input(target, input)
    }
    fn detach(&mut self) {
        self.control.detach();
        self.worker.take();
    }
}
impl Drop for PublishingDesktop {
    fn drop(&mut self) {
        self.detach();
    }
}
