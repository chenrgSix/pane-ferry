//! MIT-SHM 1.2 file-descriptor capture on the same authenticated X11 connection.
//! The immutable returned Vec is a snapshot: later server writes cannot alter an
//! in-flight encoder input. No System V segment, global key or permission change.
use anyhow::{bail, Result};
use std::{os::fd::AsRawFd, ptr::NonNull};
use x11rb::{
    connection::Connection,
    protocol::{shm::ConnectionExt as _, xproto::ImageFormat},
    rust_connection::RustConnection,
};

struct Region {
    id: u32,
    pointer: NonNull<u8>,
    length: usize,
}
// The mapping has no thread affinity. Backend serializes access through its
// session mutex; no Rust slice exists while the X server writes the next image.
unsafe impl Send for Region {}
impl Drop for Region {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.pointer.as_ptr().cast(), self.length);
        }
    }
}
pub(super) struct SharedCapture {
    enabled: bool,
    region: Option<Region>,
}
impl SharedCapture {
    pub fn new(connection: &RustConnection) -> Self {
        let enabled = (|| -> Result<bool> {
            let version = connection.shm_query_version()?.reply()?;
            Ok((version.major_version, version.minor_version) >= (1, 2))
        })()
        .unwrap_or(false);
        Self {
            enabled,
            region: None,
        }
    }
    fn disable(&mut self, connection: &RustConnection) {
        self.enabled = false;
        if let Some(region) = self.region.take() {
            if let Ok(cookie) = connection.shm_detach(region.id) {
                let _ = cookie.check();
            }
        }
    }
    fn image(
        &mut self,
        connection: &RustConnection,
        drawable: u32,
        width: u32,
        height: u32,
    ) -> Result<(u8, u32, Vec<u8>)> {
        remoteapp_protocol::dimensions(width, height)?;
        let length = width as usize * height as usize * 4;
        if self.region.as_ref().is_none_or(|r| r.length < length) {
            if let Some(old) = self.region.take() {
                connection.shm_detach(old.id)?.check()?;
            }
            let id = connection.generate_id()?;
            let segment = connection
                .shm_create_segment(id, length as u32, false)?
                .reply()?;
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            let valid_size = unsafe {
                libc::fstat(segment.shm_fd.as_raw_fd(), stat.as_mut_ptr()) == 0
                    && (*stat.as_ptr()).st_size >= length as i64
            };
            if !valid_size {
                let _ = connection.shm_detach(id)?.check();
                bail!("invalid shared X11 descriptor size")
            }
            // The server owns its writable mapping; this process needs only a
            // read-only view. The received private fd closes after mmap.
            let mapped = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    length,
                    libc::PROT_READ,
                    libc::MAP_SHARED,
                    segment.shm_fd.as_raw_fd(),
                    0,
                )
            };
            if mapped == libc::MAP_FAILED || mapped.is_null() {
                if mapped.is_null() {
                    unsafe {
                        libc::munmap(mapped, length);
                    }
                }
                let _ = connection.shm_detach(id)?.check();
                bail!("shared X11 mapping unavailable")
            }
            let pointer = NonNull::new(mapped.cast::<u8>())
                .ok_or_else(|| anyhow::anyhow!("null shared X11 mapping"))?;
            self.region = Some(Region {
                id,
                pointer,
                length,
            });
        }
        let region = self.region.as_ref().unwrap();
        let reply = connection
            .shm_get_image(
                drawable,
                0,
                0,
                width as u16,
                height as u16,
                u32::MAX,
                ImageFormat::Z_PIXMAP.into(),
                region.id,
                0,
            )?
            .reply()?;
        let size = reply.size as usize;
        if size == 0 || size > region.length {
            bail!("invalid shared X11 image size")
        }
        // GetImage completion precedes the reply. Copy before another request
        // can reuse this region; never lend a live server-writable map to video.
        let data = unsafe { std::slice::from_raw_parts(region.pointer.as_ptr(), size) }.to_vec();
        Ok((reply.depth, reply.visual, data))
    }
    pub fn capture(
        &mut self,
        connection: &RustConnection,
        drawable: u32,
        width: u32,
        height: u32,
    ) -> Option<(u8, u32, Vec<u8>)> {
        if !self.enabled {
            return None;
        }
        match self.image(connection, drawable, width, height) {
            Ok(image) => Some(image),
            Err(_) => {
                self.disable(connection);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::protocol::xproto::{
        ConnectionExt as _, CreateGCAux, CreateWindowAux, Rectangle, WindowClass,
    };

    #[test]
    #[ignore = "requires a private Xvfb DISPLAY with MIT-SHM 1.2; run with xvfb-run"]
    fn shared_snapshots_match_socket_images_after_resize() {
        let (connection, screen) = x11rb::connect(None).unwrap();
        let root = connection.setup().roots[screen].root;
        let window = connection.generate_id().unwrap();
        connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                root,
                0,
                0,
                640,
                480,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new(),
            )
            .unwrap()
            .check()
            .unwrap();
        connection.map_window(window).unwrap().check().unwrap();
        let gc = connection.generate_id().unwrap();
        connection
            .create_gc(gc, window, &CreateGCAux::new().foreground(0x264fae))
            .unwrap()
            .check()
            .unwrap();
        let mut capture = SharedCapture::new(&connection);
        assert!(capture.enabled, "test server lacks MIT-SHM 1.2");
        let mut snapshot = None;
        for (width, height) in [(640, 480), (641, 483), (320, 200)] {
            connection
                .configure_window(
                    window,
                    &x11rb::protocol::xproto::ConfigureWindowAux::new()
                        .width(width)
                        .height(height),
                )
                .unwrap()
                .check()
                .unwrap();
            connection
                .poly_fill_rectangle(
                    window,
                    gc,
                    &[Rectangle {
                        x: 0,
                        y: 0,
                        width: width as u16,
                        height: height as u16,
                    }],
                )
                .unwrap()
                .check()
                .unwrap();
            let reference = connection
                .get_image(
                    ImageFormat::Z_PIXMAP,
                    window,
                    0,
                    0,
                    width as u16,
                    height as u16,
                    u32::MAX,
                )
                .unwrap()
                .reply()
                .unwrap();
            let (depth, visual, pixels) =
                capture.capture(&connection, window, width, height).unwrap();
            assert!(capture.enabled);
            assert_eq!(depth, reference.depth);
            assert_eq!(visual, reference.visual);
            assert_eq!(pixels, reference.data);
            if let Some((old, copy)) = &snapshot {
                assert_eq!(old, copy, "later capture mutated a previous snapshot");
            }
            snapshot = Some((pixels.clone(), pixels));
        }
        assert!(capture.capture(&connection, 0, 64, 48).is_none());
        assert!(
            !capture.enabled && capture.region.is_none(),
            "failed SHM must release mapping and select fallback"
        );
        let fallback = connection
            .get_image(ImageFormat::Z_PIXMAP, window, 0, 0, 320, 200, u32::MAX)
            .unwrap()
            .reply()
            .unwrap();
        assert!(!fallback.data.is_empty());
        connection.destroy_window(window).unwrap().check().unwrap();
    }
}
