use paneferry_transport::DesktopWindowInfo;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const CLOSE_COOLDOWN_MS: u64 = 5_000;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Off,
    All,
    Selected,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub mode: Mode,
    #[serde(default)]
    pub applications: Vec<String>,
}
impl Rule {
    pub fn validate(&self) -> Result<(), String> {
        if self.applications.len() > 128
            || self
                .applications
                .iter()
                .any(|id| paneferry_transport::validate_id(id).is_err())
        {
            return Err("自动接入应用名单无效。".into());
        }
        Ok(())
    }
    fn matches(&self, window: &DesktopWindowInfo) -> bool {
        window.auto_connectable
            && match self.mode {
                Mode::Off => false,
                Mode::All => true,
                Mode::Selected => window
                    .application_key
                    .as_ref()
                    .is_some_and(|id| self.applications.contains(id)),
            }
    }
}

#[derive(Clone)]
struct Pending {
    request_id: String,
    retry_at: u64,
}

/// One planner per paired device. All connected applications contribute rules
/// to the same union; no initiating-application inference is involved.
#[derive(Default)]
pub struct Planner {
    pub armed: bool,
    seen: HashSet<String>,
    pending: HashMap<String, Pending>,
    closed: HashMap<String, u64>,
    closed_sessions: HashMap<String, u64>,
}
impl Planner {
    pub fn disarm(&mut self) {
        self.armed = false;
        self.seen.clear();
        self.pending.clear();
        // Preserve recent dismissals across a rapid disconnect/reconnect.
    }
    pub fn baseline(&mut self, windows: &[DesktopWindowInfo]) {
        self.seen = windows.iter().map(|w| w.id.clone()).collect();
        self.pending.clear();
        self.armed = true;
    }
    pub fn dismiss(&mut self, windows: &[String], session: &str, now: u64) {
        for id in windows {
            self.pending.remove(id);
            self.seen.insert(id.clone());
            self.closed.insert(id.clone(), now + CLOSE_COOLDOWN_MS);
        }
        self.closed_sessions
            .insert(session.into(), now + CLOSE_COOLDOWN_MS);
    }
    pub fn observe(&mut self, windows: &[DesktopWindowInfo], rules: &[Rule], now: u64) {
        self.closed.retain(|_, until| *until > now);
        self.closed_sessions.retain(|_, until| *until > now);
        if !self.armed {
            self.baseline(windows);
            return;
        }
        for window in windows {
            let fresh = self.seen.insert(window.id.clone());
            let dismissed = self.closed.contains_key(&window.id)
                || window
                    .session_id
                    .as_ref()
                    .is_some_and(|s| self.closed_sessions.contains_key(s));
            if window.controlled || dismissed {
                self.pending.remove(&window.id);
            } else if fresh && !window.minimized && rules.iter().any(|r| r.matches(window)) {
                self.pending.insert(
                    window.id.clone(),
                    Pending {
                        request_id: uuid::Uuid::new_v4().to_string(),
                        retry_at: now,
                    },
                );
            }
        }
        // Keep seen identities through incomplete inventories and transient
        // reconnects. Disappearing and reappearing is not itself a new window.
        // A fresh baseline at the bound avoids unbounded lifetime accumulation.
        if self.seen.len() > 8192 {
            self.baseline(windows);
        }
        self.pending
            .retain(|id, _| windows.iter().any(|w| w.id == *id));
    }
    pub fn candidate(
        &self,
        windows: &[DesktopWindowInfo],
        rules: &[Rule],
        manual_targets: impl Iterator<Item = String>,
        now: u64,
    ) -> Option<(DesktopWindowInfo, String)> {
        let manual: HashSet<_> = manual_targets.collect();
        windows.iter().find_map(|w| {
            let pending = self.pending.get(&w.id)?;
            if pending.retry_at > now
                || w.controlled
                || w.minimized
                || self.closed.contains_key(&w.id)
                || w.session_id
                    .as_ref()
                    .is_some_and(|s| self.closed_sessions.contains_key(s))
                || manual.contains(&w.id)
                || w.app_id.as_ref().is_some_and(|id| manual.contains(id))
                || w.session_id.as_ref().is_some_and(|id| manual.contains(id))
                || !rules.iter().any(|r| r.matches(w))
            {
                return None;
            }
            Some((w.clone(), pending.request_id.clone()))
        })
    }
    pub fn finish(&mut self, id: &str, retry: bool, now: u64) {
        if retry {
            if let Some(pending) = self.pending.get_mut(id) {
                pending.retry_at = now + 5_000;
            }
        } else {
            self.pending.remove(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn window(id: &str, app: &str) -> DesktopWindowInfo {
        serde_json::from_value(
            serde_json::json!({"id":id,"app_id":app,"application_key":app,
            "auto_connectable":true,"window_id":1,"name":app,"title":"fixture",
            "icon_png":null,"minimized":false,"controlled":false}),
        )
        .unwrap()
    }
    fn all() -> Rule {
        Rule {
            mode: Mode::All,
            applications: vec![],
        }
    }
    fn selected(app: &str) -> Rule {
        Rule {
            mode: Mode::Selected,
            applications: vec![app.into()],
        }
    }
    fn next(
        p: &Planner,
        w: &[DesktopWindowInfo],
        r: &[Rule],
        now: u64,
    ) -> Option<(DesktopWindowInfo, String)> {
        p.candidate(w, r, std::iter::empty(), now)
    }
    #[test]
    fn multiple_applications_union_rules_and_connect_each_window_once() {
        let mut p = Planner::default();
        let old = window("old", "editor");
        p.observe(&[old.clone()], &[all(), all()], 0);
        assert!(next(&p, &[old.clone()], &[all()], 0).is_none());
        let windows = [old, window("new", "chooser")];
        let rules = [all(), selected("chooser")];
        p.observe(&windows, &rules, 100);
        let (w, _) = next(&p, &windows, &rules, 100).unwrap();
        assert_eq!(w.id, "new");
        p.finish(&w.id, false, 100);
        p.observe(&windows, &rules, 200);
        assert!(next(&p, &windows, &rules, 200).is_none());
    }
    #[test]
    fn selected_rules_are_a_union_and_do_not_capture_old_unmatched_windows_when_changed() {
        let mut p = Planner::default();
        p.baseline(&[]);
        let windows = [
            window("one", "chooser"),
            window("two", "browser"),
            window("three", "editor"),
        ];
        let rules = [selected("chooser"), selected("browser")];
        p.observe(&windows, &rules, 0);
        for _ in 0..2 {
            let (w, _) = next(&p, &windows, &rules, 0).unwrap();
            assert_ne!(w.id, "three");
            p.finish(&w.id, false, 0);
        }
        p.observe(&windows, &[all()], 100);
        assert!(next(&p, &windows, &[all()], 100).is_none());
    }
    #[test]
    fn explicit_launch_reservation_and_controlled_windows_win_over_automatic_rules() {
        let mut p = Planner::default();
        p.baseline(&[]);
        let mut windows = [window("new", "editor")];
        p.observe(&windows, &[all()], 0);
        assert!(p
            .candidate(&windows, &[all()], ["editor".into()].into_iter(), 0)
            .is_none());
        windows[0].controlled = true;
        p.observe(&windows, &[all()], 1);
        windows[0].controlled = false;
        p.observe(&windows, &[all()], 2);
        assert!(next(&p, &windows, &[all()], 2).is_none());
    }
    #[test]
    fn close_cooldown_survives_rearming_and_does_not_reopen_a_still_existing_window() {
        let mut p = Planner::default();
        p.baseline(&[]);
        let mut w = window("new", "chooser");
        w.session_id = Some("session".into());
        p.observe(&[w.clone()], &[all(), all()], 0);
        p.dismiss(&[w.id.clone()], "session", 100);
        p.disarm();
        p.baseline(&[]);
        p.observe(&[w.clone()], &[all()], 4999);
        assert!(next(&p, &[w.clone()], &[all()], 4999).is_none());
        p.observe(&[w.clone()], &[all()], 9000);
        assert!(next(&p, &[w], &[all()], 9000).is_none());
        let fresh = window("different", "chooser");
        p.observe(&[fresh.clone()], &[all()], 9001);
        assert!(next(&p, &[fresh], &[all()], 9001).is_some());
    }
    #[test]
    fn retry_keeps_request_id_and_legacy_popups_minimized_windows_are_not_opened() {
        let mut p = Planner::default();
        p.baseline(&[]);
        let w = window("new", "chooser");
        p.observe(&[w.clone()], &[all()], 0);
        let (_, first) = next(&p, &[w.clone()], &[all()], 0).unwrap();
        p.finish(&w.id, true, 0);
        assert!(next(&p, &[w.clone()], &[all()], 4999).is_none());
        assert_eq!(next(&p, &[w], &[all()], 5000).unwrap().1, first);
        let mut unsupported = window("tooltip", "editor");
        unsupported.auto_connectable = false;
        let mut hidden = window("hidden", "editor");
        hidden.minimized = true;
        p.observe(&[unsupported.clone(), hidden.clone()], &[all()], 6000);
        assert!(next(&p, &[unsupported, hidden], &[all()], 6000).is_none());
    }
}
