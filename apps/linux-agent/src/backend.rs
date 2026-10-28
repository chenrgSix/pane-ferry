//! The session owns capture/input implementations; transport and device leases stay shared.
use anyhow::Result;
use paneferry_backend_x11::{Backend as X11, CopiedText};
use paneferry_protocol::Window;
use std::{collections::HashMap, sync::Arc, time::Duration};
mod logical_x11;
pub use logical_x11::LogicalX11;

pub trait WindowBackend: Send {
    fn windows(&self) -> &HashMap<u32, Window>;
    fn prepare_input(&self);
    fn input_status(&self) -> Option<&'static str>;
    fn refresh_capture(&self);
    /// A fresh controller must revalidate streams whose producer may have
    /// been retired while the application itself remained alive.
    fn resume_capture(&mut self) {
        self.refresh_capture();
    }
    fn refresh_window(&self, _id: u32) {
        self.refresh_capture();
    }
    /// Video activity resumed on the same controller/lease. Discard prepared
    /// pixels without reacquiring streams or changing input ownership.
    fn refresh_video(&mut self) {
        for id in self.windows().keys().copied().collect::<Vec<_>>() {
            self.refresh_window(id);
        }
    }
    /// An explicit launch/resume may retry a previously failed size adjustment.
    fn retry_window_adjustments(&mut self) {}
    fn event_fd(&self) -> std::os::fd::RawFd;
    fn capture_completion_fd(&self) -> Option<std::os::fd::RawFd> {
        None
    }
    /// Readback admission already enforces the negotiated sampling interval.
    /// Completed immutable snapshots must not wait through another FPS gate.
    fn capture_is_paced(&self) -> bool {
        false
    }
    fn capture_delay(&self, _interval: Duration) -> Option<Duration> {
        None
    }
    fn maintenance_ready(&self) -> bool {
        true
    }
    fn events_pending(&self) -> Result<bool> {
        Ok(false)
    }
    fn existing_window_exists(&self, window: u32) -> bool;
    fn includes_window(&self, id: u32) -> bool;
    fn scan(&mut self) -> Result<Vec<Window>>;
    /// A completed background snapshot wakes the capture owner so new geometry
    /// is consumed without waiting for the next periodic scan.
    fn metadata_ready(&self) -> bool {
        false
    }
    /// Structural changes wake discovery without polling at the video cadence.
    fn metadata_events(&self) -> bool {
        false
    }
    /// Drain readiness without pixel work when false. When true, also submit
    /// and collect ready samples without waiting; publication has its own cap.
    fn pump_capture_events(&mut self, prepare_pixels: bool) -> Result<()>;
    fn prepare_capture(&mut self, _interval: Duration) -> Result<()> {
        self.pump_capture_events(true)
    }
    fn capture_pending(&self) -> bool;
    fn capture_needed(&self, window: u32) -> bool;
    fn capture(&self, window: &Window) -> Result<Arc<crate::video::Frame>>;
    fn configure(&self, id: u32, width: u32, height: u32) -> Result<()>;
    fn configure_native(&self, id: u32, width: u32, height: u32) -> Result<()> {
        self.configure(id, width, height)
    }
    fn focus(&self, id: u32) -> Result<()>;
    fn close(&self, id: u32) -> Result<()>;
    fn pointer(&self, id: u32, x: i32, y: i32, button: u8, pressed: bool) -> Result<()>;
    fn key(&self, id: u32, keysym: u32, pressed: bool) -> Result<()>;
    fn release_all(&self);
    fn copy_text(&mut self, window: u32, id: String) -> Result<()>;
    fn cut_text(&mut self, window: u32, id: String) -> Result<()>;
    fn copy_bundle(&mut self, window: u32, id: String, cut: bool) -> Result<()>;
    fn cut_pending(&self) -> bool;
    fn pump_copy(&mut self) -> Result<Option<CopiedText>>;
    fn text_pending(&self) -> bool;
    fn cancel_text(&mut self);
    fn commit_text(&mut self, window: u32, text: String) -> Result<()>;
    fn paste_text(&mut self, window: u32, text: String) -> Result<()> {
        self.commit_text(window, text)
    }
    fn paste_bundle(&mut self, window: u32, transfer: &str) -> Result<()>;
    fn pump_text(&mut self) -> Result<()>;
}
impl WindowBackend for X11 {
    fn windows(&self) -> &HashMap<u32, Window> {
        &self.windows
    }
    fn prepare_input(&self) {
        X11::prepare_input(self)
    }
    fn input_status(&self) -> Option<&'static str> {
        X11::input_status(self)
    }
    fn refresh_capture(&self) {
        X11::refresh_capture(self)
    }
    fn refresh_window(&self, id: u32) {
        X11::refresh_window(self, id)
    }
    fn retry_window_adjustments(&mut self) {
        X11::retry_window_adjustments(self)
    }
    fn event_fd(&self) -> std::os::fd::RawFd {
        X11::event_fd(self)
    }
    fn capture_completion_fd(&self) -> Option<std::os::fd::RawFd> {
        X11::capture_completion_fd(self)
    }
    fn capture_is_paced(&self) -> bool {
        true
    }
    fn capture_delay(&self, interval: Duration) -> Option<Duration> {
        X11::capture_delay(self, interval)
    }
    fn maintenance_ready(&self) -> bool {
        !X11::capture_busy(self)
    }
    fn events_pending(&self) -> Result<bool> {
        X11::events_pending(self)
    }
    fn existing_window_exists(&self, window: u32) -> bool {
        X11::existing_window_exists(self, window)
    }
    fn includes_window(&self, id: u32) -> bool {
        X11::includes_window(self, id)
    }
    fn scan(&mut self) -> Result<Vec<Window>> {
        X11::scan(self)
    }
    fn metadata_ready(&self) -> bool {
        X11::metadata_ready(self)
    }
    fn metadata_events(&self) -> bool {
        X11::metadata_events(self)
    }
    fn pump_capture_events(&mut self, _prepare_pixels: bool) -> Result<()> {
        X11::pump_capture_events(self)
    }
    fn prepare_capture(&mut self, interval: Duration) -> Result<()> {
        X11::pump_capture_events(self)?;
        X11::prepare_capture(self, interval)
    }
    fn capture_pending(&self) -> bool {
        X11::capture_pending(self)
    }
    fn capture_needed(&self, window: u32) -> bool {
        X11::capture_needed(self, window)
    }
    fn capture(&self, window: &Window) -> Result<Arc<crate::video::Frame>> {
        X11::take_capture(self, window, window.width, window.height)
            .map(|image| Arc::new(crate::video::Frame::from_image(image)))
    }
    fn configure(&self, id: u32, width: u32, height: u32) -> Result<()> {
        X11::configure(self, id, width, height)
    }
    fn focus(&self, id: u32) -> Result<()> {
        X11::focus(self, id)
    }
    fn close(&self, id: u32) -> Result<()> {
        X11::close(self, id)
    }
    fn pointer(&self, id: u32, x: i32, y: i32, button: u8, pressed: bool) -> Result<()> {
        X11::pointer(self, id, x, y, button, pressed)
    }
    fn key(&self, id: u32, keysym: u32, pressed: bool) -> Result<()> {
        X11::key(self, id, keysym, pressed)
    }
    fn release_all(&self) {
        X11::release_all(self)
    }
    fn copy_text(&mut self, window: u32, id: String) -> Result<()> {
        X11::copy_text(self, window, id)
    }
    fn cut_text(&mut self, window: u32, id: String) -> Result<()> {
        X11::cut_text(self, window, id)
    }
    fn copy_bundle(&mut self, window: u32, id: String, cut: bool) -> Result<()> {
        X11::copy_bundle(self, window, id, cut)
    }
    fn cut_pending(&self) -> bool {
        X11::cut_pending(self)
    }
    fn pump_copy(&mut self) -> Result<Option<CopiedText>> {
        X11::pump_copy(self)
    }
    fn text_pending(&self) -> bool {
        X11::text_pending(self)
    }
    fn cancel_text(&mut self) {
        X11::cancel_text(self)
    }
    fn commit_text(&mut self, window: u32, text: String) -> Result<()> {
        X11::commit_text(self, window, text)
    }
    fn paste_bundle(&mut self, window: u32, transfer: &str) -> Result<()> {
        X11::paste_bundle(self, window, transfer)
    }
    fn pump_text(&mut self) -> Result<()> {
        X11::pump_text(self)
    }
}
