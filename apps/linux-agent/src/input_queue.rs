use remoteapp_protocol::{Request, MAX_TEXT_BYTES};
use std::{
    io::Read,
    os::{
        fd::{AsRawFd, RawFd},
        unix::net::UnixStream,
    },
    sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError},
    time::Duration,
};

/// Wait for either request readiness, X11 events or the next metadata/frame
/// deadline. The wake socket is nonblocking; the channel remains authoritative.
pub fn receive(
    receiver: &Receiver<Request>,
    wake: &mut UnixStream,
    x11: Option<RawFd>,
    timeout: Duration,
) -> Result<Request, RecvTimeoutError> {
    match receiver.try_recv() {
        Ok(request) => return Ok(request),
        Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
        Err(TryRecvError::Empty) => {}
    }
    let mut events = [
        libc::pollfd {
            fd: wake.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: x11.unwrap_or(-1),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    let milliseconds =
        (timeout.as_micros().saturating_add(999) / 1000).min(i32::MAX as u128) as i32;
    unsafe {
        libc::poll(
            events.as_mut_ptr(),
            events.len() as libc::nfds_t,
            milliseconds,
        );
    }
    let mut bytes = [0; 256];
    while matches!(wake.read(&mut bytes),Ok(n) if n>0) {}
    receiver.try_recv().map_err(|error| match error {
        TryRecvError::Empty => RecvTimeoutError::Timeout,
        TryRecvError::Disconnected => RecvTimeoutError::Disconnected,
    })
}

/// Merge only consecutive committed text for one window. Keep key/click/paste
/// boundaries intact and never mutate a clipboard transfer already in flight.
pub fn coalesce(
    request: Request,
    receiver: &Receiver<Request>,
    pending: &mut Option<Request>,
) -> Request {
    let Request::Text {
        window_id,
        mut text,
    } = request
    else {
        return request;
    };
    while let Ok(next) = receiver.try_recv() {
        match next {
            Request::Text {
                window_id: next_window,
                text: more,
            } if next_window == window_id && text.len() + more.len() <= MAX_TEXT_BYTES => {
                text.push_str(&more)
            }
            barrier => {
                *pending = Some(barrier);
                break;
            }
        }
    }
    Request::Text { window_id, text }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    #[test]
    fn readiness_wakes_requests_and_preserves_messages_after_coalesced_signals() {
        use std::{io::Write, thread, time::Instant};
        let (sender, receiver) = mpsc::channel();
        let (mut wake, mut notify) = UnixStream::pair().unwrap();
        wake.set_nonblocking(true).unwrap();
        notify.set_nonblocking(true).unwrap();
        let thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            for _ in 0..3 {
                sender.send(Request::List {}).unwrap();
            }
            notify.write_all(&[1]).unwrap();
            sender
        });
        let started = Instant::now();
        assert!(matches!(
            receive(&receiver, &mut wake, None, Duration::from_secs(3)),
            Ok(Request::List {})
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
        for _ in 0..2 {
            assert!(matches!(
                receive(&receiver, &mut wake, None, Duration::ZERO),
                Ok(Request::List {})
            ));
        }
        drop(thread.join().unwrap());
        assert!(matches!(
            receive(&receiver, &mut wake, None, Duration::ZERO),
            Err(RecvTimeoutError::Disconnected)
        ));
    }
    #[test]
    fn keeps_keys_paste_and_other_windows_as_ordering_barriers() {
        for barrier in [
            Request::Key {
                window_id: 1,
                keysym: 0xff08,
                pressed: true,
            },
            Request::Paste {
                window_id: 1,
                text: "explicit".into(),
            },
            Request::Cut {
                window_id: 1,
                request_id: "cut".into(),
            },
            Request::Text {
                window_id: 2,
                text: "other".into(),
            },
        ] {
            let (sender, receiver) = mpsc::channel();
            sender
                .send(Request::Text {
                    window_id: 1,
                    text: "文".into(),
                })
                .unwrap();
            sender.send(barrier).unwrap();
            sender
                .send(Request::Text {
                    window_id: 1,
                    text: "later".into(),
                })
                .unwrap();
            let mut pending = None;
            let combined = coalesce(
                Request::Text {
                    window_id: 1,
                    text: "a".into(),
                },
                &receiver,
                &mut pending,
            );
            assert!(matches!(combined, Request::Text { text, .. } if text == "a文"));
            assert!(pending.is_some());
            assert!(
                matches!(receiver.try_recv().unwrap(), Request::Text { text, .. } if text == "later")
            );
        }
    }
    #[test]
    fn byte_budget_preserves_overflow_as_the_next_request() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Request::Text {
                window_id: 1,
                text: "🙂".into(),
            })
            .unwrap();
        let mut pending = None;
        let text = "a".repeat(MAX_TEXT_BYTES - 1);
        let combined = coalesce(
            Request::Text { window_id: 1, text },
            &receiver,
            &mut pending,
        );
        assert!(matches!(combined, Request::Text { text, .. } if text.len() == MAX_TEXT_BYTES - 1));
        assert!(matches!(pending, Some(Request::Text { text, .. }) if text == "🙂"));
    }
}
