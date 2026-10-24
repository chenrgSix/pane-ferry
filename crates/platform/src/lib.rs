//! User-private storage and local IPC. The network wire format stays unchanged.
pub mod ipc;
#[cfg(windows)]
mod security;
pub mod storage;

#[cfg(windows)]
pub fn process_is_current_user(pid: u32) -> bool {
    security::same_process_user(pid)
}
