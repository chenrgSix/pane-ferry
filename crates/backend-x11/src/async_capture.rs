//! One private readback connection and one in-flight job per backend. No X11
//! readback reply, render operation or pixel copy runs under the input/session lock.
use crate::{readback::Surface, related_windows::Identity, *};
use std::{
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError},
    thread,
    time::Duration,
};
use x11rb::protocol::res::{ClientIdMask, ClientIdSpec, ConnectionExt as _};

const PIXEL_BUDGET: usize = 16_000_000;

pub(super) enum ConnectionSource {
    Display(String),
    Existing(u16, PathBuf),
}
impl ConnectionSource {
    fn connect(&self) -> Result<RustConnection> {
        match self {
            Self::Display(display) => Ok(x11rb::connect(Some(display))?.0),
            Self::Existing(display, authority) => authenticated_connection(*display, authority),
        }
    }
}

#[derive(Clone)]
struct Job {
    surface: Surface,
    epoch: u64,
    owner: Option<Identity>,
}
struct Finished {
    job: Job,
    image: Result<Arc<CapturedImage>, String>,
}
struct Worker {
    submit: Option<SyncSender<Job>>,
    completed: Receiver<Finished>,
    wake: UnixStream,
    interrupt: Option<OwnedFd>,
}
impl Worker {
    fn new(source: &ConnectionSource, root: u32, composite: bool) -> Result<Self> {
        let conn = source.connect()?;
        if composite {
            conn.composite_query_version(0, 4)?.reply()?;
            conn.res_query_version(1, 2)?.reply()?;
        }
        let fd = unsafe { libc::fcntl(conn.stream().as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        anyhow::ensure!(fd >= 0, "cannot prepare capture cancellation");
        let interrupt = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut shared = shared_capture::SharedCapture::new(&conn);
        let mut render = render_scale::RenderScale::new(&conn, root);
        Self::spawn(Some(interrupt), move |job| {
            let validate = || -> Result<()> {
                if let Some(owner) = &job.owner {
                    anyhow::ensure!(owner.current(), "capture process changed");
                    let reply = conn
                        .res_query_client_ids(&[ClientIdSpec {
                            client: job.surface.window,
                            mask: ClientIdMask::LOCAL_CLIENT_PID,
                        }])?
                        .reply()?;
                    anyhow::ensure!(
                        reply.ids.iter().find_map(|id| id.value.first().copied())
                            == Some(owner.pid),
                        "capture window owner changed"
                    );
                }
                Ok(())
            };
            validate()?;
            let image = readback::capture(&conn, &mut shared, &mut render, &job.surface)?;
            validate()?;
            Ok(image)
        })
    }
    fn spawn(
        interrupt: Option<OwnedFd>,
        mut capture: impl FnMut(&Job) -> Result<CapturedImage> + Send + 'static,
    ) -> Result<Self> {
        let (submit, requests) = mpsc::sync_channel::<Job>(1);
        let (results, completed) = mpsc::sync_channel(1);
        let (wake, mut notify) = UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        notify.set_nonblocking(true)?;
        thread::Builder::new()
            .name("x11-readback".into())
            .spawn(move || {
                while let Ok(job) = requests.recv() {
                    let image = capture(&job).map(Arc::new).map_err(|e| e.to_string());
                    if results.send(Finished { job, image }).is_err() {
                        break;
                    }
                    // A full socket is already readable. Never hold up the
                    // worker, or the controller, on a notification byte.
                    let _ = notify.write(&[1]);
                }
            })?;
        Ok(Self {
            submit: Some(submit),
            completed,
            wake,
            interrupt,
        })
    }
    fn drain_wake(&self) {
        let mut bytes = [0; 64];
        while matches!((&self.wake).read(&mut bytes), Ok(n) if n > 0) {}
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.submit.take();
        if let Some(fd) = &self.interrupt {
            // Cancel only our private connection. Drop must not join a thread
            // waiting for the X server, or close the input/clipboard connection.
            unsafe { libc::shutdown(fd.as_raw_fd(), libc::SHUT_RDWR) };
        }
    }
}

struct Slot {
    surface: Surface,
    epoch: u64,
    blocked: bool,
    image: Option<Arc<CapturedImage>>,
    last_started: Option<Instant>,
}
#[derive(Default)]
struct Frames {
    slots: HashMap<u32, Slot>,
    sizes: HashMap<u32, (u32, u32)>,
    active: Option<Job>,
    cursor: u32,
    next_epoch: u64,
    pixels: usize,
}
impl Frames {
    fn epoch(&mut self) -> u64 {
        self.next_epoch = self.next_epoch.wrapping_add(1).max(1);
        self.next_epoch
    }
    fn remove(&mut self, id: u32) {
        if let Some(slot) = self.slots.remove(&id) {
            self.pixels -= slot.image.map_or(0, |image| image.pixels());
        }
    }
    fn invalidate(&mut self, id: u32, block: bool) {
        let epoch = self.epoch();
        if let Some(slot) = self.slots.get_mut(&id) {
            self.pixels -= slot.image.take().map_or(0, |image| image.pixels());
            slot.epoch = epoch;
            slot.blocked |= block;
        }
    }
    fn finish(&mut self, done: Finished, valid: bool) -> bool {
        let id = done.job.surface.window;
        // There is exactly one worker job; invalidating its epoch must still
        // release its admission slot when that old job eventually finishes.
        self.active = None;
        let Some(slot) = self.slots.get_mut(&id) else {
            return false;
        };
        if !valid
            || slot.blocked
            || slot.epoch != done.job.epoch
            || slot.surface != done.job.surface
        {
            return false;
        }
        let Ok(image) = done.image else { return false };
        if (image.width, image.height) != slot.surface.target
            && (image.width, image.height) != slot.surface.source
        {
            return false;
        }
        let old = slot.image.as_ref().map_or(0, |image| image.pixels());
        if self.pixels - old + image.pixels() > PIXEL_BUDGET {
            return false;
        }
        self.pixels = self.pixels - old + image.pixels();
        slot.image = Some(image);
        true
    }
}

pub(super) struct AsyncCapture {
    worker: Worker,
    frames: RefCell<Frames>,
}
impl AsyncCapture {
    pub(super) fn pending(&self) -> bool {
        self.frames
            .borrow()
            .slots
            .values()
            .any(|slot| !slot.blocked && slot.image.is_some())
    }
    pub(super) fn needed(&self, id: u32) -> bool {
        self.frames
            .borrow()
            .slots
            .get(&id)
            .is_some_and(|slot| !slot.blocked && slot.image.is_some())
    }
}
impl Backend {
    /// Enable before acquiring a controller. Authentication and initial X11
    /// setup happen here, never in response to an input or a capture deadline.
    pub fn enable_async_capture(&mut self) -> Result<()> {
        if self.async_capture.is_none() {
            let source = self
                .connection_source
                .as_ref()
                .context("capture connection unavailable")?;
            self.async_capture = Some(AsyncCapture {
                worker: Worker::new(source, self.root, self.process_id.is_some())?,
                frames: RefCell::new(Frames::default()),
            });
            self.sync_async_capture();
        }
        Ok(())
    }
    pub(super) fn sync_async_capture(&self) {
        let Some(capture) = &self.async_capture else {
            return;
        };
        let mut frames = capture.frames.borrow_mut();
        let removed: Vec<_> = frames
            .slots
            .keys()
            .copied()
            .filter(|id| !self.windows.get(id).is_some_and(|w| w.visible))
            .collect();
        for id in removed {
            frames.remove(id);
        }
        for w in self.windows.values().filter(|w| w.visible) {
            let surface = Surface {
                window: w.window_id,
                source: (w.width, w.height),
                target: frames
                    .sizes
                    .get(&w.window_id)
                    .copied()
                    .unwrap_or((w.width, w.height)),
                visual: self.visuals.get(&w.window_id).copied(),
                composite: self.process_id.is_some(),
            };
            if frames
                .slots
                .get(&w.window_id)
                .is_some_and(|slot| slot.surface == surface)
            {
                frames.slots.get_mut(&w.window_id).unwrap().blocked = false;
            } else {
                frames.remove(w.window_id);
                let epoch = frames.epoch();
                frames.slots.insert(
                    w.window_id,
                    Slot {
                        surface,
                        epoch,
                        blocked: false,
                        image: None,
                        last_started: None,
                    },
                );
            }
        }
        frames.sizes.retain(|id, _| self.windows.contains_key(id));
    }
    pub fn set_capture_sizes(&self, sizes: HashMap<u32, (u32, u32)>) {
        if let Some(capture) = &self.async_capture {
            capture.frames.borrow_mut().sizes = sizes;
            self.sync_async_capture();
        }
    }
    pub(super) fn invalidate_async_capture(&self, id: Option<u32>, block: bool) {
        let Some(capture) = &self.async_capture else {
            return;
        };
        let mut frames = capture.frames.borrow_mut();
        let ids = id
            .map(|id| vec![id])
            .unwrap_or_else(|| frames.slots.keys().copied().collect());
        for id in ids {
            frames.invalidate(id, block);
        }
    }
    pub fn capture_completion_fd(&self) -> Option<std::os::fd::RawFd> {
        self.async_capture
            .as_ref()
            .map(|c| c.worker.wake.as_raw_fd())
    }
    pub fn capture_busy(&self) -> bool {
        self.async_capture
            .as_ref()
            .is_some_and(|c| c.frames.borrow().active.is_some())
    }
    pub fn capture_delay(&self, interval: Duration) -> Option<Duration> {
        let capture = self.async_capture.as_ref()?;
        let frames = capture.frames.borrow();
        if frames.active.is_some() || capture.pending() {
            return None;
        }
        frames
            .slots
            .values()
            .filter(|s| !s.blocked && self.damage.borrow().needed(s.surface.window))
            .map(|s| {
                s.last_started.map_or(Duration::ZERO, |last| {
                    interval.saturating_sub(last.elapsed())
                })
            })
            .min()
    }
    pub(super) fn poll_async_capture(&self) -> Result<()> {
        let Some(capture) = &self.async_capture else {
            return Ok(());
        };
        capture.worker.drain_wake();
        loop {
            let done = match capture.worker.completed.try_recv() {
                Ok(done) => done,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => bail!("X11 capture worker stopped"),
            };
            let id = done.job.surface.window;
            let valid = self.known(id).is_ok();
            if !capture.frames.borrow_mut().finish(done, valid) {
                self.damage.borrow_mut().retry(id);
            }
        }
        Ok(())
    }
    pub fn prepare_capture(&self, interval: Duration) -> Result<()> {
        self.poll_async_capture()?;
        let Some(capture) = &self.async_capture else {
            return Ok(());
        };
        // Publish an already completed snapshot before asking the X server to
        // read another one. Input/metadata still need server replies, so starting
        // a readback here can otherwise block consumption of its predecessor.
        if capture.pending()
            || self.metadata_ready()
            || self
                .capture_delay(interval)
                .is_none_or(|delay| !delay.is_zero())
        {
            return Ok(());
        }
        let mut frames = capture.frames.borrow_mut();
        let mut candidates: Vec<_> = frames
            .slots
            .keys()
            .copied()
            .filter(|id| {
                !frames.slots[id].blocked
                    && self.damage.borrow().needed(*id)
                    && frames.slots[id]
                        .last_started
                        .is_none_or(|last| last.elapsed() >= interval)
            })
            .collect();
        candidates.sort_unstable_by_key(|id| (*id <= frames.cursor, *id));
        let Some(id) = candidates.first().copied() else {
            return Ok(());
        };
        self.known(id)?;
        let owner = if self.process_id.is_some() {
            Some(Identity::read(self.client_pid(id)?)?.0)
        } else {
            None
        };
        let slot = &frames.slots[&id];
        let job = Job {
            surface: slot.surface.clone(),
            epoch: slot.epoch,
            owner,
        };
        // Rearm on the event connection before submitting to the independent
        // readback connection. New Damage during the job must survive completion.
        self.damage.borrow_mut().acknowledge(&self.conn, id)?;
        frames.slots.get_mut(&id).unwrap().last_started = Some(Instant::now());
        frames.cursor = id;
        capture
            .worker
            .submit
            .as_ref()
            .unwrap()
            .try_send(job.clone())
            .map_err(|_| anyhow::anyhow!("X11 capture worker unavailable"))?;
        frames.active = Some(job);
        Ok(())
    }
    pub fn take_capture(&self, w: &Window, width: u32, height: u32) -> Result<Arc<CapturedImage>> {
        let capture = self
            .async_capture
            .as_ref()
            .context("asynchronous capture required")?;
        anyhow::ensure!(
            self.windows.get(&w.window_id) == Some(w),
            "stale window geometry"
        );
        let mut frames = capture.frames.borrow_mut();
        let slot = frames
            .slots
            .get_mut(&w.window_id)
            .context("frame pending")?;
        anyhow::ensure!(
            !slot.blocked && slot.surface.target == (width, height),
            "frame pending"
        );
        let image = slot.image.take().context("frame pending")?;
        frames.pixels -= image.pixels();
        Ok(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::protocol::Event;

    fn image() -> CapturedImage {
        CapturedImage {
            width: 1,
            height: 1,
            data: vec![0; 4],
            bpp: 32,
            pad: 32,
            little: true,
            masks: (0xff0000, 0xff00, 0xff),
        }
    }
    fn job() -> Job {
        Job {
            surface: Surface {
                window: 7,
                source: (1, 1),
                target: (1, 1),
                visual: None,
                composite: false,
            },
            epoch: 1,
            owner: None,
        }
    }
    #[test]
    fn dropping_a_stalled_reader_cancels_only_its_private_connection() -> Result<()> {
        let (mut reader, _server) = UnixStream::pair()?;
        let interrupt: OwnedFd = reader.try_clone()?.into();
        let (entered, started) = mpsc::channel();
        let (left, exited) = mpsc::channel();
        let worker = Worker::spawn(Some(interrupt), move |_| {
            entered.send(())?;
            let result = reader.read_exact(&mut [0]);
            left.send(result.is_err())?;
            Ok(image())
        })?;
        worker.submit.as_ref().unwrap().try_send(job())?;
        started.recv_timeout(Duration::from_secs(2))?;
        let (mut control, mut peer) = UnixStream::pair()?;
        drop(worker);
        assert!(exited.recv_timeout(Duration::from_secs(2))?);
        control.write_all(&[42])?;
        let mut bytes = [0];
        peer.read_exact(&mut bytes)?;
        assert_eq!(bytes, [42]);
        Ok(())
    }

    fn wait(mut condition: impl FnMut() -> Result<bool>) -> Result<()> {
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            if condition()? {
                return Ok(());
            }
            anyhow::ensure!(
                Instant::now() < until,
                "asynchronous capture did not advance"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }
    fn fixture() -> Result<(Backend, RustConnection, Window)> {
        let (painter, screen) = x11rb::connect(None)?;
        let root = painter.setup().roots[screen].root;
        let id = painter.generate_id()?;
        painter
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                id,
                root,
                0,
                0,
                320,
                240,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new().event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE),
            )?
            .check()?;
        painter.map_window(id)?.check()?;
        let clients = painter
            .intern_atom(false, b"_NET_CLIENT_LIST")?
            .reply()?
            .atom;
        painter
            .change_property32(PropMode::REPLACE, root, clients, AtomEnum::WINDOW, &[id])?
            .check()?;
        let mut backend = Backend::connect(&std::env::var("DISPLAY")?)?;
        backend.scan()?;
        let window = backend.windows[&id].clone();
        Ok((backend, painter, window))
    }
    fn install_delayed_reader(
        backend: &mut Backend,
        after_pixels: bool,
    ) -> Result<(Receiver<()>, SyncSender<()>)> {
        let source = backend.connection_source.as_ref().unwrap();
        let conn = source.connect()?;
        let mut shared = shared_capture::SharedCapture::new(&conn);
        let mut render = render_scale::RenderScale::new(&conn, backend.root);
        let (entered, started) = mpsc::sync_channel(1);
        let (release, gate) = mpsc::sync_channel(1);
        let mut first = true;
        let worker = Worker::spawn(None, move |job| {
            let result = if after_pixels {
                Some(readback::capture(
                    &conn,
                    &mut shared,
                    &mut render,
                    &job.surface,
                )?)
            } else {
                None
            };
            if first {
                first = false;
                entered.send(())?;
                gate.recv_timeout(Duration::from_secs(3))?;
            }
            result.map_or_else(
                || readback::capture(&conn, &mut shared, &mut render, &job.surface),
                Ok,
            )
        })?;
        backend.async_capture = Some(AsyncCapture {
            worker,
            frames: RefCell::new(Frames::default()),
        });
        backend.sync_async_capture();
        Ok((started, release))
    }
    fn paint(painter: &RustConnection, id: u32, width: u16, height: u16, color: u32) -> Result<()> {
        let gc = painter.generate_id()?;
        painter
            .create_gc(gc, id, &CreateGCAux::new().foreground(color))?
            .check()?;
        painter
            .poly_fill_rectangle(
                id,
                gc,
                &[Rectangle {
                    x: 0,
                    y: 0,
                    width,
                    height,
                }],
            )?
            .check()?;
        painter.free_gc(gc)?.check()?;
        Ok(())
    }

    #[test]
    #[ignore = "requires a private Xvfb DISPLAY with XTEST, DAMAGE and SHM; run serially"]
    fn stalled_readback_keeps_keyboard_live_and_rejects_a_resized_result() -> Result<()> {
        let (mut backend, painter, window) = fixture()?;
        let (started, release) = install_delayed_reader(&mut backend, false)?;
        backend.prepare_capture(Duration::from_micros(33_333))?;
        started.recv_timeout(Duration::from_secs(2))?;
        assert!(backend.capture_busy());
        assert_eq!(backend.capture_delay(Duration::from_micros(33_333)), None);
        for _ in 0..128 {
            backend.prepare_capture(Duration::from_micros(33_333))?;
        }
        backend.key(window.window_id, 0x61, true)?;
        backend.key(window.window_id, 0x61, false)?;
        let mut keys = vec![];
        wait(|| {
            while let Some(event) = painter.poll_for_event()? {
                match event {
                    Event::KeyPress(_) => keys.push(true),
                    Event::KeyRelease(_) => keys.push(false),
                    _ => {}
                }
            }
            Ok(keys.len() >= 2)
        })?;
        assert_eq!(
            keys,
            [true, false],
            "both events arrive while readback is still gated"
        );
        assert!(backend.capture_busy());
        // This fixture has no WM; the source application resizes its own window.
        painter
            .configure_window(
                window.window_id,
                &ConfigureWindowAux::new().width(321).height(243),
            )?
            .check()?;
        paint(&painter, window.window_id, 321, 243, 0x00ff00)?;
        backend.pump_capture_events()?;
        backend.scan()?;
        let current = backend.windows[&window.window_id].clone();
        assert_eq!((current.width, current.height), (321, 243));
        release.send(())?;
        wait(|| {
            backend.pump_capture_events()?;
            Ok(!backend.capture_busy())
        })?;
        assert!(
            !backend.capture_pending(),
            "an old-size completion must never be published"
        );
        wait(|| {
            backend.pump_capture_events()?;
            backend.prepare_capture(Duration::from_micros(33_333))?;
            Ok(backend.capture_pending())
        })?;
        let snapshot = backend.take_capture(&current, 321, 243)?;
        assert_eq!((snapshot.width, snapshot.height), (321, 243));
        assert_eq!(&snapshot.data[..3], &[0, 255, 0]);
        paint(&painter, window.window_id, 321, 243, 0x0000ff)?;
        wait(|| {
            backend.pump_capture_events()?;
            backend.prepare_capture(Duration::from_micros(33_333))?;
            Ok(backend.capture_pending())
        })?;
        let later = backend.take_capture(&current, 321, 243)?;
        assert_eq!(&later.data[..3], &[255, 0, 0]);
        assert_eq!(
            &snapshot.data[..3],
            &[0, 255, 0],
            "SHM reuse must not change a published snapshot"
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires a private Xvfb DISPLAY with DAMAGE and SHM; run serially"]
    fn controller_refresh_discards_an_in_flight_snapshot_and_retains_new_damage() -> Result<()> {
        let (mut backend, painter, window) = fixture()?;
        paint(&painter, window.window_id, 320, 240, 0xff0000)?;
        let (started, release) = install_delayed_reader(&mut backend, true)?;
        backend.prepare_capture(Duration::from_micros(33_333))?;
        started.recv_timeout(Duration::from_secs(2))?;
        paint(&painter, window.window_id, 320, 240, 0x00ff00)?;
        backend.refresh_capture();
        release.send(())?;
        wait(|| {
            backend.pump_capture_events()?;
            Ok(!backend.capture_busy())
        })?;
        assert!(
            !backend.capture_pending(),
            "a previous controller's pixels are not reusable"
        );
        wait(|| {
            backend.pump_capture_events()?;
            backend.prepare_capture(Duration::from_micros(33_333))?;
            Ok(backend.capture_pending())
        })?;
        let snapshot = backend.take_capture(&window, 320, 240)?;
        assert_eq!(&snapshot.data[..3], &[0, 255, 0]);
        Ok(())
    }

    #[test]
    #[ignore = "requires a private Xvfb DISPLAY with DAMAGE and SHM; run serially"]
    fn completed_snapshot_is_consumed_before_admitting_the_next_readback() -> Result<()> {
        let (mut backend, painter, window) = fixture()?;
        paint(&painter, window.window_id, 320, 240, 0xff0000)?;
        backend.enable_async_capture()?;
        backend.prepare_capture(Duration::ZERO)?;
        wait(|| {
            backend.pump_capture_events()?;
            Ok(backend.capture_pending())
        })?;
        paint(&painter, window.window_id, 320, 240, 0x00ff00)?;
        backend.pump_capture_events()?;
        for _ in 0..128 {
            backend.prepare_capture(Duration::ZERO)?;
            assert!(
                !backend.capture_busy(),
                "ready pixels precede another readback"
            );
            assert_eq!(backend.capture_delay(Duration::ZERO), None);
        }
        let first = backend.take_capture(&window, 320, 240)?;
        assert_eq!(&first.data[..3], &[0, 0, 255]);
        wait(|| {
            backend.pump_capture_events()?;
            backend.prepare_capture(Duration::ZERO)?;
            Ok(backend.capture_pending())
        })?;
        let second = backend.take_capture(&window, 320, 240)?;
        assert_eq!(&second.data[..3], &[0, 255, 0]);
        Ok(())
    }

    #[test]
    #[ignore = "requires a private Xvfb DISPLAY with XRES; run serially"]
    fn compositor_bound_input_does_not_query_a_grabbed_x_server() -> Result<()> {
        struct Input(mpsc::Sender<&'static str>);
        impl DesktopInput for Input {
            fn prepare(&self) {}
            fn status(&self) -> &'static str {
                "ready"
            }
            fn pointer(&self, _: i32, _: i32, _: u8, _: bool) -> Result<()> {
                self.0.send("pointer")?;
                Ok(())
            }
            fn key(&self, _: u8, pressed: bool) -> Result<()> {
                self.0.send(if pressed { "press" } else { "release" })?;
                Ok(())
            }
            fn sync(&self) -> Result<()> {
                Ok(())
            }
            fn release_all(&self) {}
        }
        struct Geometry(u32);
        impl DesktopGeometry for Geometry {
            fn input_target(&self, id: u32, _: bool) -> Result<bool> {
                anyhow::ensure!(id == self.0, "unknown window");
                Ok(true)
            }
            fn resize(&self, _: u32, _: u32, _: u32) -> Result<bool> {
                Ok(false)
            }
            fn pointer(
                &self,
                id: u32,
                _: i32,
                _: i32,
                _: u32,
                _: u32,
            ) -> Result<Option<(i32, i32)>> {
                anyhow::ensure!(id == self.0, "unknown window");
                Ok(Some((10, 20)))
            }
        }
        let (mut backend, painter, window) = fixture()?;
        let pid = std::process::id();
        let display = std::env::var("DISPLAY")?
            .trim_start_matches(':')
            .split('.')
            .next()
            .unwrap()
            .parse()?;
        backend.window_scope = Some(related_windows::Scope::new(
            display,
            pid,
            window.window_id,
            &[],
        )?);
        let (events, received) = mpsc::channel();
        backend.set_desktop_input(Arc::new(Input(events)));
        backend.set_desktop_geometry(Box::new(Geometry(window.window_id)));
        painter.grab_server()?.check()?;
        let (finished, done) = mpsc::channel();
        let worker = thread::spawn(move || {
            let result = (|| -> Result<()> {
                backend.key(window.window_id, 0x61, true)?;
                backend.key(window.window_id, 0x61, false)?;
                backend.pointer(window.window_id, 10, 20, 1, true)?;
                backend.windows.get_mut(&window.window_id).unwrap().visible = false;
                assert_eq!(
                    backend
                        .key(window.window_id, 0x61, true)
                        .unwrap_err()
                        .to_string(),
                    "window is hidden"
                );
                assert_eq!(
                    backend
                        .pointer(window.window_id, 10, 20, 1, true)
                        .unwrap_err()
                        .to_string(),
                    "window is hidden"
                );
                Ok(())
            })();
            let _ = finished.send(result);
        });
        let result = done.recv_timeout(Duration::from_millis(500));
        // Always release the server before asserting, including a regression.
        painter.ungrab_server()?.check()?;
        worker.join().unwrap();
        result.context("input waited for the grabbed X server")??;
        assert_eq!(
            received.try_iter().collect::<Vec<_>>(),
            ["press", "release", "pointer"]
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires a private Xvfb DISPLAY; run serially"]
    fn keyboard_mapping_notifications_invalidate_cached_keycodes() -> Result<()> {
        let (backend, painter, _) = fixture()?;
        let code = painter.setup().min_keycode;
        let old = painter.get_keyboard_mapping(code, 1)?.reply()?;
        let symbol = 0x123456;
        painter
            .change_keyboard_mapping(1, code, 1, &[symbol])?
            .check()?;
        let changed = (|| -> Result<()> {
            wait(|| {
                backend.pump_capture_events()?;
                Ok(backend.keymap.borrow().is_none())
            })?;
            assert_eq!(backend.keycode(symbol)?, code);
            Ok(())
        })();
        painter
            .change_keyboard_mapping(1, code, old.keysyms_per_keycode, &old.keysyms)?
            .check()?;
        changed
    }

    #[test]
    #[ignore = "requires a private Linux Xvfb DISPLAY with XRES 1.2; run serially"]
    fn readback_revalidates_the_server_reported_owner_before_reading_pixels() -> Result<()> {
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let (backend, _painter, window) = fixture()?;
        let other = Child(std::process::Command::new("sleep").arg("5").spawn()?);
        let worker = Worker::new(
            backend.connection_source.as_ref().unwrap(),
            backend.root,
            true,
        )?;
        let request = Job {
            surface: Surface {
                window: window.window_id,
                source: (320, 240),
                target: (320, 240),
                visual: backend.visuals.get(&window.window_id).copied(),
                composite: false,
            },
            epoch: 1,
            owner: Some(Identity::read(other.0.id())?.0),
        };
        worker.submit.as_ref().unwrap().try_send(request)?;
        let result = worker.completed.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(result.image.unwrap_err(), "capture window owner changed");
        Ok(())
    }
}
