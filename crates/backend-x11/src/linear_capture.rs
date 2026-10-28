//! Optional private, reusable linear render target. Only this backend's own
//! destination is mapped, never an application or desktop buffer. GPU completion
//! and CPU coherency are separate; snapshots remain immutable owned bytes.
use crate::*;

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::{
        ffi::{c_int, c_uint, c_void},
        os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
        ptr::{self, NonNull},
        sync::{Arc, Mutex, OnceLock},
        time::Duration,
    };
    use x11rb::protocol::dri3::ConnectionExt as _;
    use x11rb::protocol::sync::ConnectionExt as _;

    type Object = *mut c_void;
    struct Api {
        device: unsafe extern "C" fn(c_int) -> Object,
        free_device: unsafe extern "C" fn(Object),
        buffer: unsafe extern "C" fn(Object, c_uint, c_uint, c_uint, c_uint) -> Object,
        free_buffer: unsafe extern "C" fn(Object),
        fd: unsafe extern "C" fn(Object) -> c_int,
        stride: unsafe extern "C" fn(Object) -> c_uint,
        modifier: unsafe extern "C" fn(Object) -> u64,
    }
    fn api() -> Option<&'static Api> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        API.get_or_init(|| unsafe {
            let library = libc::dlopen(c"libgbm.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
            if library.is_null() {
                return None;
            }
            macro_rules! symbol {
                ($name:literal, $ty:ty) => {{
                    let p = libc::dlsym(library, concat!($name, "\0").as_ptr().cast());
                    if p.is_null() {
                        libc::dlclose(library);
                        return None;
                    }
                    std::mem::transmute::<Object, $ty>(p)
                }};
            }
            // Keep the library loaded while any device/BO destructor can run.
            Some(Api {
                device: symbol!("gbm_create_device", unsafe extern "C" fn(c_int) -> Object),
                free_device: symbol!("gbm_device_destroy", unsafe extern "C" fn(Object)),
                buffer: symbol!(
                    "gbm_bo_create",
                    unsafe extern "C" fn(Object, c_uint, c_uint, c_uint, c_uint) -> Object
                ),
                free_buffer: symbol!("gbm_bo_destroy", unsafe extern "C" fn(Object)),
                fd: symbol!("gbm_bo_get_fd", unsafe extern "C" fn(Object) -> c_int),
                stride: symbol!("gbm_bo_get_stride", unsafe extern "C" fn(Object) -> c_uint),
                modifier: symbol!("gbm_bo_get_modifier", unsafe extern "C" fn(Object) -> u64),
            })
        })
        .as_ref()
    }
    fn private(fd: &OwnedFd) -> Result<()> {
        let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
        anyhow::ensure!(
            flags >= 0 && flags & libc::FD_CLOEXEC != 0,
            "render descriptor must be close-on-exec"
        );
        Ok(())
    }
    struct Inner {
        api: &'static Api,
        raw: Object,
        _fd: OwnedFd,
        lock: Mutex<()>,
    }
    // GBM calls and BO destruction are serialized by lock. The owner outlives
    // every BO, even if a private target is moved to a different worker thread.
    unsafe impl Send for Inner {}
    unsafe impl Sync for Inner {}
    pub(crate) struct Device(Arc<Inner>);
    impl Device {
        pub fn new(conn: &RustConnection, root: u32) -> Option<Self> {
            (|| -> Result<Self> {
                let api = api().context("GBM unavailable")?;
                conn.dri3_query_version(1, 0)?.reply()?;
                let version = conn.sync_initialize(3, 1)?.reply()?;
                anyhow::ensure!(
                    (version.major_version, version.minor_version) >= (3, 1),
                    "render fences unavailable"
                );
                let fd = conn.dri3_open(root, 0)?.reply()?.device_fd;
                private(&fd)?;
                let raw = unsafe { (api.device)(fd.as_raw_fd()) };
                anyhow::ensure!(!raw.is_null(), "GBM device unavailable");
                Ok(Self(Arc::new(Inner {
                    api,
                    raw,
                    _fd: fd,
                    lock: Mutex::new(()),
                })))
            })()
            .ok()
        }
        pub fn buffer(
            &self,
            conn: &RustConnection,
            root: u32,
            id: u32,
            size: (u32, u32),
        ) -> Result<Buffer> {
            paneferry_protocol::dimensions(size.0, size.1)?;
            const XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
            const RENDERING_LINEAR: u32 = (1 << 2) | (1 << 4);
            let _lock = self.0.lock.lock().unwrap();
            let api = self.0.api;
            let bo = unsafe {
                (api.buffer)(
                    self.0.raw,
                    size.0.next_multiple_of(16),
                    size.1 + size.1 % 2,
                    XRGB8888,
                    RENDERING_LINEAR,
                )
            };
            anyhow::ensure!(!bo.is_null(), "linear render target unavailable");
            struct Guard(&'static Api, Object);
            impl Drop for Guard {
                fn drop(&mut self) {
                    unsafe { (self.0.free_buffer)(self.1) };
                }
            }
            let guard = Guard(api, bo);
            anyhow::ensure!(
                unsafe { (api.modifier)(bo) } == 0,
                "render target is not linear"
            );
            let stride = unsafe { (api.stride)(bo) };
            let length = stride
                .checked_mul(size.1 + size.1 % 2)
                .context("invalid render target")? as usize;
            anyhow::ensure!(
                stride >= size.0 * 4 && stride <= u32::from(u16::MAX),
                "invalid render stride"
            );
            let fd = unsafe { (api.fd)(bo) };
            anyhow::ensure!(fd >= 0, "render target export unavailable");
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            private(&fd)?;
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            anyhow::ensure!(
                unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } == 0,
                "invalid render descriptor"
            );
            let stat = unsafe { stat.assume_init() };
            anyhow::ensure!(
                stat.st_size >= length as i64 && stat.st_ino != 0,
                "short render buffer"
            );
            let buffer = Buffer {
                owner: self.0.clone(),
                bo: guard.1,
                fd,
                map: Mutex::new(None),
                length,
                stride,
                size,
                device: stat.st_dev,
                inode: stat.st_ino,
                fence: None,
            };
            std::mem::forget(guard);
            drop(_lock);
            conn.dri3_pixmap_from_buffer(
                id,
                root,
                length as u32,
                (size.0 + size.0 % 2) as u16,
                (size.1 + size.1 % 2) as u16,
                stride as u16,
                24,
                32,
                buffer.fd.try_clone()?,
            )?
            .check()?;
            Ok(buffer)
        }
    }
    impl Drop for Inner {
        fn drop(&mut self) {
            unsafe { (self.api.free_device)(self.raw) };
        }
    }
    pub(crate) struct Buffer {
        owner: Arc<Inner>,
        bo: Object,
        fd: OwnedFd,
        map: Mutex<Option<NonNull<u8>>>,
        length: usize,
        stride: u32,
        size: (u32, u32),
        device: libc::dev_t,
        inode: libc::ino_t,
        fence: Option<u32>,
    }
    // The pool obtains &mut Buffer only when Arc::get_mut proves no leases
    // exist. Once leased, only immutable metadata / serialized read maps are
    // accessible. GBM destruction is serialized by the device mutex.
    unsafe impl Send for Buffer {}
    unsafe impl Sync for Buffer {}
    fn sync(fd: c_int, end: bool) -> Result<()> {
        // Linux DMA_BUF_IOCTL_SYNC = _IOW('b',0,struct dma_buf_sync).
        let request = 0x4008_6200 as libc::c_ulong;
        let flags: u64 = if end { 5 } else { 1 }; // READ | END/START
        loop {
            if unsafe { libc::ioctl(fd, request, &flags) } == 0 {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(libc::EINTR | libc::EAGAIN)) {
                continue;
            }
            return Err(error.into());
        }
    }
    impl Buffer {
        pub fn size(&self) -> (u32, u32) {
            self.size
        }
        pub fn stride(&self) -> u32 {
            self.stride
        }
        pub fn length(&self) -> usize {
            self.length
        }
        pub fn fd(&self) -> BorrowedFd<'_> {
            self.fd.as_fd()
        }
        pub fn retire(&self, conn: &RustConnection) {
            if let Some(fence) = self.fence {
                let _ = conn.sync_destroy_fence(fence);
            }
        }
        pub fn snapshot(&mut self, conn: &RustConnection, pixmap: u32) -> Result<Vec<u8>> {
            self.ready(conn, pixmap)?;
            self.read_pixels()
        }
        pub fn ready(&mut self, conn: &RustConnection, pixmap: u32) -> Result<()> {
            // Glamor's XSync fence trigger flushes queued GL rendering. Only
            // then can the dma-buf poll below see the current GPU write fence.
            let reset = if let Some(fence) = self.fence {
                Some(conn.sync_reset_fence(fence)?)
            } else {
                let fence = conn.generate_id()?;
                conn.sync_create_fence(pixmap, fence, false)?.check()?;
                self.fence = Some(fence);
                None
            };
            let trigger = conn.sync_trigger_fence(self.fence.unwrap())?;
            let awaited = conn.sync_await_fence(&[self.fence.unwrap()])?;
            // Also verify export still names our imported object, not a server
            // replacement. Its reply covers the preceding trigger/reset.
            let ready = conn.dri3_buffer_from_pixmap(pixmap)?.reply()?;
            awaited.check()?;
            trigger.check()?;
            if let Some(reset) = reset {
                reset.check()?;
            }
            anyhow::ensure!(
                (u32::from(ready.width), u32::from(ready.height))
                    == (self.size.0 + self.size.0 % 2, self.size.1 + self.size.1 % 2)
                    && u32::from(ready.stride) == self.stride
                    && ready.bpp == 32
                    && ready.depth == 24,
                "render target layout changed"
            );
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            anyhow::ensure!(
                unsafe { libc::fstat(ready.pixmap_fd.as_raw_fd(), stat.as_mut_ptr()) } == 0,
                "invalid render export"
            );
            let stat = unsafe { stat.assume_init() };
            anyhow::ensure!(
                (stat.st_dev, stat.st_ino) == (self.device, self.inode),
                "render target replaced"
            );
            let end = Instant::now() + Duration::from_secs(1);
            let mut event = libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            loop {
                let remaining = end.saturating_duration_since(Instant::now());
                anyhow::ensure!(!remaining.is_zero(), "render fence timeout");
                let n = unsafe { libc::poll(&mut event, 1, remaining.as_millis().max(1) as c_int) };
                if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                anyhow::ensure!(
                    n > 0
                        && event.revents & libc::POLLIN != 0
                        && event.revents & (libc::POLLERR | libc::POLLNVAL) == 0,
                    "render fence unavailable"
                );
                break;
            }
            Ok(())
        }
        pub fn read_pixels(&self) -> Result<Vec<u8>> {
            let mut mapping = self.map.lock().unwrap();
            if mapping.is_none() {
                let mapped = unsafe {
                    libc::mmap(
                        ptr::null_mut(),
                        self.length,
                        libc::PROT_READ,
                        libc::MAP_SHARED,
                        self.fd.as_raw_fd(),
                        0,
                    )
                };
                anyhow::ensure!(
                    mapped != libc::MAP_FAILED && !mapped.is_null(),
                    "linear mapping unavailable"
                );
                *mapping = NonNull::new(mapped.cast());
            }
            let map = mapping.unwrap();
            sync(self.fd.as_raw_fd(), false)?;
            struct End(c_int);
            impl Drop for End {
                fn drop(&mut self) {
                    let _ = sync(self.0, true);
                }
            }
            let end = End(self.fd.as_raw_fd());
            let row = self.size.0 as usize * 4;
            let mut pixels = Vec::with_capacity(row * self.size.1 as usize);
            for y in 0..self.size.1 as usize {
                let src = unsafe {
                    std::slice::from_raw_parts(map.as_ptr().add(y * self.stride as usize), row)
                };
                pixels.extend_from_slice(src);
            }
            sync(self.fd.as_raw_fd(), true)?;
            std::mem::forget(end);
            Ok(pixels)
        }
    }
    impl Drop for Buffer {
        fn drop(&mut self) {
            if let Some(map) = *self.map.get_mut().unwrap() {
                unsafe { libc::munmap(map.as_ptr().cast(), self.length) };
            }
            let _lock = self.owner.lock.lock().unwrap();
            unsafe { (self.owner.api.free_buffer)(self.bo) };
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) use platform::{Buffer, Device};

#[cfg(not(target_os = "linux"))]
pub(super) struct Device;
#[cfg(not(target_os = "linux"))]
pub(super) struct Buffer;
#[cfg(not(target_os = "linux"))]
impl Device {
    pub fn new(_: &RustConnection, _: u32) -> Option<Self> {
        None
    }
    pub fn buffer(&self, _: &RustConnection, _: u32, _: u32, _: (u32, u32)) -> Result<Buffer> {
        bail!("linear capture unavailable")
    }
}
#[cfg(not(target_os = "linux"))]
impl Buffer {
    pub fn snapshot(&mut self, _: &RustConnection, _: u32) -> Result<Vec<u8>> {
        bail!("linear capture unavailable")
    }
    pub fn retire(&self, _: &RustConnection) {}
    pub fn ready(&mut self, _: &RustConnection, _: u32) -> Result<()> {
        bail!("linear capture unavailable")
    }
    pub fn read_pixels(&self) -> Result<Vec<u8>> {
        bail!("linear capture unavailable")
    }
    pub fn size(&self) -> (u32, u32) {
        (0, 0)
    }
    pub fn stride(&self) -> u32 {
        0
    }
    pub fn length(&self) -> usize {
        0
    }
    pub fn fd(&self) -> std::os::fd::BorrowedFd<'_> {
        unreachable!("no linear buffers on this platform")
    }
}
