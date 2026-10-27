//! Window membership is pinned to OS process identities, never window titles or
//! client-supplied _NET_WM_PID. Independent desktop windows remain out of scope.
use anyhow::{bail, Context, Result};
use std::{
    collections::{HashMap, HashSet},
    fs,
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Identity {
    pub(super) pid: u32,
    started: u64,
    executable: PathBuf,
}
impl Identity {
    pub(super) fn read(pid: u32) -> Result<(Self, u32)> {
        let root = PathBuf::from(format!("/proc/{pid}"));
        if fs::metadata(&root)?.uid() != unsafe { libc::geteuid() } {
            bail!("foreign process")
        }
        let stat = fs::read_to_string(root.join("stat"))?;
        let fields: Vec<_> = stat
            .rsplit_once(')')
            .context("invalid process")?
            .1
            .split_whitespace()
            .collect();
        Ok((
            Self {
                pid,
                started: fields.get(19).context("missing start")?.parse()?,
                executable: fs::read_link(root.join("exe"))?,
            },
            fields.get(1).context("missing parent")?.parse()?,
        ))
    }
    pub(super) fn current(&self) -> bool {
        Self::read(self.pid).is_ok_and(|(now, _)| now == *self)
    }
    fn descendant_of(&self, root: &Self) -> bool {
        let mut current = self.clone();
        let mut seen = HashSet::new();
        for _ in 0..32 {
            if current == *root {
                return root.current();
            }
            if !seen.insert(current.pid) {
                return false;
            }
            let Ok((actual, parent)) = Self::read(current.pid) else {
                return false;
            };
            if actual != current || parent <= 1 {
                return false;
            }
            let Ok((next, _)) = Self::read(parent) else {
                return false;
            };
            if next.started > current.started {
                return false;
            }
            current = next;
        }
        false
    }
}

#[derive(Clone, Copy)]
pub(super) struct Candidate {
    pub id: u32,
    pub pid: u32,
    pub parent: Option<u32>,
    pub visible: bool,
}
struct Member {
    identity: Identity,
    parent: Option<u32>,
}
#[derive(Default)]
struct Registry {
    claims: HashMap<(u16, u32), u64>,
}
fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

pub(super) struct Scope {
    token: u64,
    display: u16,
    root: Identity,
    root_window: u32,
    members: HashMap<u32, Member>,
    retired: HashSet<u32>,
}
impl Scope {
    pub fn new(display: u16, pid: u32, window: u32, _initial: &[Candidate]) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = Identity::read(pid)?.0;
        let token = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut registry = registry().lock().unwrap();
        if registry.claims.contains_key(&(display, window)) {
            bail!("session already controlled")
        }
        registry.claims.insert((display, window), token);
        Ok(Self {
            token,
            display,
            root: root.clone(),
            root_window: window,
            members: [(
                window,
                Member {
                    identity: root,
                    parent: None,
                },
            )]
            .into(),
            retired: HashSet::new(),
        })
    }
    pub fn contains(&self, id: u32, pid: u32) -> bool {
        self.members.get(&id).is_some_and(|m| {
            m.identity.pid == pid && m.identity.current() && m.identity.descendant_of(&self.root)
        })
    }
    pub fn current(&self, id: u32) -> bool {
        self.members
            .get(&id)
            .is_some_and(|m| self.contains(id, m.identity.pid))
    }
    /// The identity approved by discovery. Pixel reads must still validate the
    /// X server's current owner on their private connection before and after
    /// readback; this only avoids resolving the same owner on the input thread.
    pub fn capture_identity(&self, id: u32) -> Option<Identity> {
        self.current(id)
            .then(|| self.members.get(&id).unwrap().identity.clone())
    }
    pub fn parent(&self, id: u32) -> Option<u32> {
        self.members.get(&id).and_then(|m| m.parent)
    }
    pub fn clipboard_pid(&self, id: u32, claimed: u32) -> Option<u32> {
        let member = self.members.get(&id)?;
        let candidate = Identity::read(claimed).ok()?.0;
        // Electron's GPU window may delegate selections to its main process.
        // Accept that hint only after proving it is a live ancestor in this scope.
        (candidate.descendant_of(&self.root) && member.identity.descendant_of(&candidate))
            .then_some(claimed)
    }
    pub fn update(&mut self, candidates: &[Candidate]) {
        let live: HashMap<_, _> = candidates.iter().map(|w| (w.id, w.pid)).collect();
        let mut registry = registry().lock().unwrap();
        for id in self.members.keys().copied().collect::<Vec<_>>() {
            if id != self.root_window && !live.get(&id).is_some_and(|pid| self.contains(id, *pid)) {
                self.members.remove(&id);
                self.retired.insert(id);
                if registry.claims.get(&(self.display, id)) == Some(&self.token) {
                    registry.claims.remove(&(self.display, id));
                }
            }
        }
        for _ in 0..16 {
            let orphaned: Vec<_> = self
                .members
                .iter()
                .filter_map(|(id, member)| {
                    member
                        .parent
                        .filter(|parent| !self.members.contains_key(parent))
                        .map(|_| *id)
                })
                .collect();
            if orphaned.is_empty() {
                break;
            }
            for id in orphaned {
                self.members.remove(&id);
                self.retired.insert(id);
                if registry.claims.get(&(self.display, id)) == Some(&self.token) {
                    registry.claims.remove(&(self.display, id));
                }
            }
        }
        // An absent XID has crossed an observed destruction boundary. Old
        // network frames are rejected by the control watermark; a newly created
        // window must independently pass identity + explicit-parent checks.
        // Retain only still-live orphaned windows, never a lifetime ID counter.
        self.retired.retain(|id| live.contains_key(id));
        // Iterate to admit nested explicit dialogs after their parent. A changed
        // XID or process is never silently rebound to an existing member.
        for _ in 0..16 {
            let mut added = false;
            for c in candidates {
                if self.members.contains_key(&c.id)
                    || self.members.len() >= 16
                    || self.retired.contains(&c.id)
                {
                    continue;
                }
                let Ok((identity, _)) = Identity::read(c.pid) else {
                    continue;
                };
                if !identity.descendant_of(&self.root) {
                    continue;
                }
                let current_member =
                    |id: &u32| live.get(id).is_some_and(|pid| self.contains(*id, *pid));
                // Input timing and executable equality are not ownership.
                // Unrelated main windows and unparented previews need their own
                // explicit selection, even when created by the same process.
                let Some(parent) = c.parent.filter(current_member) else {
                    continue;
                };
                if registry
                    .claims
                    .get(&(self.display, c.id))
                    .is_some_and(|owner| *owner != self.token)
                {
                    continue;
                }
                registry.claims.insert((self.display, c.id), self.token);
                self.members.insert(
                    c.id,
                    Member {
                        identity,
                        parent: Some(parent),
                    },
                );
                added = true;
            }
            if !added {
                break;
            }
        }
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        let mut registry = registry().lock().unwrap();
        registry.claims.retain(|_, owner| *owner != self.token);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    fn window(id: u32, parent: Option<u32>) -> Candidate {
        Candidate {
            id,
            pid: std::process::id(),
            parent,
            visible: true,
        }
    }
    #[test]
    fn live_orphans_stay_excluded_but_new_ids_can_be_revalidated() {
        let root = window(1, None);
        let mut scope = Scope::new(61001, root.pid, root.id, &[root]).unwrap();
        let child = window(2, Some(1));
        let grandchild = window(3, Some(2));
        scope.update(&[grandchild, child, root]);
        assert!(scope.contains(3, root.pid));
        scope.update(&[root, grandchild]);
        assert!(!scope.contains(2, root.pid));
        assert!(!scope.contains(3, root.pid));
        scope.update(&[root, child, grandchild]);
        assert!(scope.contains(2, root.pid));
        assert!(!scope.contains(3, root.pid));
        scope.update(&[root, child]);
        scope.update(&[root, child, grandchild]);
        assert!(scope.contains(3, root.pid));
        drop(scope);
        assert!(Scope::new(61001, root.pid, root.id, &[root]).is_ok());
    }
    #[test]
    fn same_process_unparented_windows_never_expand_a_selected_scope() {
        let alpha = window(11, None);
        let beta = window(12, None);
        let preview = window(13, None);
        let mut a = Scope::new(61002, alpha.pid, alpha.id, &[alpha, beta]).unwrap();
        let mut b = Scope::new(61002, beta.pid, beta.id, &[alpha, beta]).unwrap();
        a.update(&[alpha, beta, preview]);
        b.update(&[alpha, beta, preview]);
        assert!(!a.contains(beta.id, beta.pid));
        assert!(!a.contains(preview.id, preview.pid));
        assert!(!b.contains(preview.id, preview.pid));
        let ambiguous = window(14, None);
        a.update(&[alpha, beta, preview, ambiguous]);
        b.update(&[alpha, beta, preview, ambiguous]);
        assert!(!a.contains(preview.id, preview.pid));
        assert!(!a.contains(ambiguous.id, ambiguous.pid));
        assert!(!b.contains(ambiguous.id, ambiguous.pid));
    }
    #[test]
    fn sequential_dialogs_do_not_exhaust_a_lifetime_budget() {
        let root = window(1, None);
        let mut scope = Scope::new(61004, root.pid, root.id, &[root]).unwrap();
        for id in 2..=513 {
            scope.update(&[root, window(id, Some(root.id))]);
            assert!(scope.contains(id, root.pid));
            scope.update(&[root]);
            assert_eq!(scope.members.len(), 1);
            assert!(scope.retired.is_empty());
        }
    }
    #[test]
    fn kernel_identity_rejects_reuse_and_clipboard_delegation_outside_scope() {
        let (identity, parent) = Identity::read(std::process::id()).unwrap();
        let mut reused = identity.clone();
        reused.started += 1;
        assert!(identity.current());
        assert!(!reused.current());
        assert!(!reused.descendant_of(&identity));
        assert!(!identity.descendant_of(&reused));
        let root = window(21, None);
        let scope = Scope::new(61003, root.pid, root.id, &[root]).unwrap();
        assert_eq!(scope.clipboard_pid(root.id, root.pid), Some(root.pid));
        assert_eq!(scope.clipboard_pid(root.id, parent), None);
    }
    #[test]
    fn capture_identity_is_live_scoped_and_pinned_to_process_start() {
        let root = window(31, None);
        let mut scope = Scope::new(61006, root.pid, root.id, &[root]).unwrap();
        let approved = scope.capture_identity(root.id).unwrap();
        assert_eq!(approved.pid, root.pid);
        assert!(scope.capture_identity(root.id + 1).is_none());
        scope.members.get_mut(&root.id).unwrap().identity.started += 1;
        assert!(
            scope.capture_identity(root.id).is_none(),
            "a changed process incarnation cannot authorize a capture"
        );
    }
}
