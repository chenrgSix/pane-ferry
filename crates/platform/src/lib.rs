//! User-private storage and local IPC. The network wire format stays unchanged.
pub mod ipc;
#[cfg(windows)]
mod security;
pub mod storage;
