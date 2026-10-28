//! Immutable leases on this backend's private render targets, never on an
//! application's buffers. A target cannot be rendered again while leased.
use crate::{linear_capture::Buffer, CapturedImage};
use anyhow::{Context, Result};
use std::{
    os::fd::BorrowedFd,
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, OnceLock},
    time::Instant,
};

#[derive(Debug)]
pub enum VideoCapture {
    Cpu(Arc<CapturedImage>),
    DmaBuf(Arc<DmaBufFrame>),
}
impl From<CapturedImage> for VideoCapture {
    fn from(image: CapturedImage) -> Self {
        Self::Cpu(Arc::new(image))
    }
}
impl VideoCapture {
    pub fn width(&self) -> u32 {
        match self {
            Self::Cpu(i) => i.width,
            Self::DmaBuf(i) => i.width(),
        }
    }
    pub fn height(&self) -> u32 {
        match self {
            Self::Cpu(i) => i.height,
            Self::DmaBuf(i) => i.height(),
        }
    }
    pub fn pixels(&self) -> usize {
        self.width() as usize * self.height() as usize
    }
    pub fn cpu_image(&self) -> Result<Arc<CapturedImage>> {
        match self {
            Self::Cpu(i) => Ok(i.clone()),
            Self::DmaBuf(i) => i.cpu_image(),
        }
    }
}

pub struct DmaBufFrame {
    buffer: Arc<Buffer>,
    snapshot: OnceLock<Result<Arc<CapturedImage>, String>>,
    capture_started: Instant,
    capture_finished: Instant,
    preference: Option<Arc<AtomicBool>>,
}
impl std::fmt::Debug for DmaBufFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DmaBufFrame")
            .field("width", &self.width())
            .field("height", &self.height())
            .finish()
    }
}
impl DmaBufFrame {
    pub(crate) fn new(buffer: Arc<Buffer>) -> Self {
        Self {
            buffer,
            snapshot: OnceLock::new(),
            capture_started: Instant::now(),
            capture_finished: Instant::now(),
            preference: None,
        }
    }
    pub fn width(&self) -> u32 {
        self.buffer.size().0
    }
    pub fn height(&self) -> u32 {
        self.buffer.size().1
    }
    pub fn stride(&self) -> u32 {
        self.buffer.stride()
    }
    pub fn length(&self) -> usize {
        self.buffer.length()
    }
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.buffer.fd()
    }
    pub(crate) fn start(&mut self, started: Instant) {
        self.capture_started = started;
    }
    pub fn elapsed_ms(&self) -> f64 {
        self.capture_started.elapsed().as_secs_f64() * 1000.0
    }
    pub fn capture_ms(&self) -> f64 {
        self.capture_finished
            .duration_since(self.capture_started)
            .as_secs_f64()
            * 1000.0
    }
    pub(crate) fn preference(&mut self, preference: Arc<AtomicBool>) {
        self.preference = Some(preference);
    }
    pub fn prefer_software(&self) {
        if let Some(preference) = &self.preference {
            preference.store(true, Ordering::Release);
        }
    }
    // Used only by a software fallback on the encoding worker. The render
    // fence has completed and the BO stays immutable for this entire lease.
    pub fn cpu_image(&self) -> Result<Arc<CapturedImage>> {
        self.snapshot
            .get_or_init(|| {
                self.buffer
                    .read_pixels()
                    .map(|data| {
                        Arc::new(CapturedImage {
                            width: self.width(),
                            height: self.height(),
                            data,
                            bpp: 32,
                            pad: 32,
                            little: true,
                            masks: (0xff0000, 0xff00, 0xff),
                        })
                    })
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .cloned()
            .map_err(|e| anyhow::anyhow!(e.clone()))
            .context("GPU frame readback failed")
    }
}
