//! Private application sessions end when their last real window is destroyed,
//! even if a toolkit keeps its process alive. Hidden windows are still alive.
use paneferry_protocol::Window;
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct WindowLifetime {
    windows: HashSet<u32>,
    seen_window: bool,
    absent_since: Option<Instant>,
    last_scan: Option<Instant>,
}

impl WindowLifetime {
    pub fn scan_due(&self, now: Instant) -> bool {
        self.last_scan
            .is_none_or(|last| now.duration_since(last) >= Duration::from_millis(100))
    }

    pub fn scan_failed(&mut self, now: Instant) {
        self.last_scan = Some(now);
        self.absent_since = None;
    }

    pub fn observe(
        &mut self,
        windows: &[Window],
        now: Instant,
        exists: impl Fn(u32) -> bool,
    ) -> bool {
        self.last_scan = Some(now);
        let current: HashSet<_> = windows
            .iter()
            .filter(|w| w.role != "popup")
            .map(|w| w.window_id)
            .collect();
        self.seen_window |= !current.is_empty();
        self.windows.extend(&current);
        // A scan can omit a live window during resize or withdrawal. Confirm
        // destruction instead of treating visibility or capture loss as exit.
        self.windows
            .retain(|id| current.contains(id) || exists(*id));
        if !self.seen_window || !self.windows.is_empty() {
            self.absent_since = None;
            return false;
        }
        // Allow applications to replace their initial/main window.
        now.duration_since(*self.absent_since.get_or_insert(now)) >= Duration::from_secs(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn window(id: u32, role: &str, visible: bool) -> Window {
        Window {
            window_id: id,
            parent_id: None,
            title: String::new(),
            role: role.into(),
            x: 0,
            y: 0,
            width: 640,
            height: 480,
            scale_factor: 1,
            geometry_revision: 1,
            visible,
        }
    }
    #[test]
    fn hidden_and_temporarily_omitted_windows_keep_the_session() {
        let mut state = WindowLifetime::default();
        let now = Instant::now();
        assert!(!state.observe(&[], now, |_| false));
        assert!(!state.observe(&[window(1, "toplevel", true)], now, |_| true));
        assert!(!state.observe(
            &[window(1, "toplevel", false)],
            now + Duration::from_secs(5),
            |_| true
        ));
        assert!(!state.observe(&[], now + Duration::from_secs(10), |id| id == 1));
        assert!(!state.observe(&[], now + Duration::from_secs(20), |id| id == 1));
    }
    #[test]
    fn last_destroyed_window_ends_session_despite_cached_popups() {
        let mut state = WindowLifetime::default();
        let now = Instant::now();
        let popup = window(9, "popup", false);
        assert!(!state.observe(
            &[window(1, "toplevel", true), window(2, "toplevel", true)],
            now,
            |_| true
        ));
        assert!(!state.observe(&[window(2, "toplevel", true)], now, |id| id == 2));
        assert!(!state.observe(std::slice::from_ref(&popup), now, |_| false));
        assert!(state.observe(&[popup], now + Duration::from_secs(1), |_| false));
    }
    #[test]
    fn replacement_windows_and_failed_scans_cancel_exit_deadline() {
        let mut state = WindowLifetime::default();
        let now = Instant::now();
        assert!(!state.observe(&[window(1, "toplevel", true)], now, |_| true));
        assert!(!state.observe(&[], now, |_| false));
        assert!(!state.observe(
            &[window(2, "dialog", true)],
            now + Duration::from_millis(500),
            |_| false
        ));
        assert!(!state.observe(&[], now + Duration::from_secs(2), |_| false));
        state.scan_failed(now + Duration::from_secs(3));
        assert!(!state.observe(&[], now + Duration::from_secs(4), |_| false));
        assert!(state.observe(&[], now + Duration::from_secs(5), |_| false));
    }
}
