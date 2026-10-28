//! Diagnostic only: inspect a snapshot of the private target's write fences.
//! Existing poll and DMA_BUF_IOCTL_SYNC remain the synchronization authority.
use crate::publisher_timing::{detail_value, now};
use serde_json::json;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

#[repr(C)]
struct Export {
    flags: u32,
    fd: i32,
}
#[repr(C)]
#[derive(Default)]
struct FileInfo {
    name: [u8; 32],
    status: i32,
    flags: u32,
    count: u32,
    pad: u32,
    entries: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct FenceInfo {
    object: [u8; 32],
    driver: [u8; 32],
    status: i32,
    flags: u32,
    timestamp: u64,
}

pub(super) fn export(fd: i32) -> Option<OwnedFd> {
    assert_eq!(std::mem::size_of::<Export>(), 8);
    assert_eq!(std::mem::size_of::<FileInfo>(), 56);
    assert_eq!(std::mem::size_of::<FenceInfo>(), 80);
    let mut request = Export { flags: 1, fd: -1 };
    let begin = now();
    let status = unsafe { libc::ioctl(fd, 0xc008_6202 as libc::c_ulong, &mut request) };
    if status < 0 {
        detail_value(
            json!({"stage":"linear.fence_export_error","timestamp_ns":now(),
            "errno":std::io::Error::last_os_error().raw_os_error()}),
        );
        return None;
    }
    let fence = unsafe { OwnedFd::from_raw_fd(request.fd) };
    let flags = unsafe { libc::fcntl(fence.as_raw_fd(), libc::F_GETFD) };
    assert!(
        flags >= 0 && flags & libc::FD_CLOEXEC != 0,
        "diagnostic fence must be close-on-exec"
    );
    detail_value(json!({"stage":"linear.fence_export","timestamp_ns":now(),"begin_ns":begin}));
    Some(fence)
}

pub(super) fn observe(fence: Option<&OwnedFd>) {
    let Some(fence) = fence else { return };
    let mut info = FileInfo::default();
    let status = unsafe { libc::ioctl(fence.as_raw_fd(), 0xc038_3e04 as libc::c_ulong, &mut info) };
    if status < 0 || info.count > 64 {
        detail_value(
            json!({"stage":"linear.fence_info_error","timestamp_ns":now(),"result":status,"count":info.count}),
        );
        return;
    }
    let mut entries = vec![
        FenceInfo {
            object: [0; 32],
            driver: [0; 32],
            status: 0,
            flags: 0,
            timestamp: 0
        };
        info.count as usize
    ];
    info.entries = entries.as_mut_ptr() as u64;
    let status = unsafe { libc::ioctl(fence.as_raw_fd(), 0xc038_3e04 as libc::c_ulong, &mut info) };
    if status < 0 {
        return;
    }
    let signals: Vec<_> = entries
        .iter()
        .map(|f| json!({"status":f.status,"signal_ns":f.timestamp}))
        .collect();
    detail_value(
        json!({"stage":"linear.fence_info","timestamp_ns":now(),"status":info.status,"signals":signals}),
    );
}
