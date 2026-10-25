use paneferry_protocol::{Request, MAX_TEXT_BYTES};
use std::{
    io::Read,
    os::{
        fd::{AsRawFd, RawFd},
        unix::net::UnixStream,
    },
    sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError},
    time::Duration,
};

/// Process input first, including a barrier retained by coalesce(), but admit a
/// due frame even under continuous input. Metadata still has a 100 ms bound.
pub fn defer_maintenance(
    receiver: &Receiver<Request>,
    pending: &mut Option<Request>,
    since_scan: Duration,
    frame_due: bool,
) -> bool {
    if frame_due || since_scan >= Duration::from_millis(100) {
        return false;
    }
    if pending.is_none() {
        *pending = receiver.try_recv().ok();
    }
    pending.is_some()
}

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

/// Keep the newest consecutive motion or merge committed text for one window.
/// Key/click/scroll/paste and window boundaries remain ordered. Limit each turn
/// so a producer cannot keep coalescing forever without dispatch or capture.
pub fn coalesce(
    mut request: Request,
    receiver: &Receiver<Request>,
    pending: &mut Option<Request>,
) -> Request {
    if !matches!(
        &request,
        Request::Text { .. }
            | Request::Pointer {
                button: 0,
                pressed: false,
                ..
            }
    ) {
        return request;
    }
    for _ in 1..256 {
        let Ok(next) = receiver.try_recv() else { break };
        match (&mut request, next) {
            (
                Request::Pointer {
                    window_id,
                    x,
                    y,
                    button: 0,
                    pressed: false,
                },
                Request::Pointer {
                    window_id: next_window,
                    x: next_x,
                    y: next_y,
                    button: 0,
                    pressed: false,
                },
            ) if *window_id == next_window => {
                *x = next_x;
                *y = next_y;
            }
            (
                Request::Text { window_id, text },
                Request::Text {
                    window_id: next_window,
                    text: more,
                },
            ) if *window_id == next_window && text.len() + more.len() <= MAX_TEXT_BYTES => {
                text.push_str(&more)
            }
            (_, barrier) => {
                *pending = Some(barrier);
                break;
            }
        }
    }
    request
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    #[test]
    fn queued_input_precedes_capture_but_cannot_starve_maintenance() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Request::Key {
                window_id: 1,
                keysym: 42,
                pressed: false,
            })
            .unwrap();
        let mut pending = Some(Request::Key {
            window_id: 1,
            keysym: 42,
            pressed: true,
        });
        assert!(defer_maintenance(
            &receiver,
            &mut pending,
            Duration::from_millis(20),
            false
        ));
        assert!(matches!(
            pending.take(),
            Some(Request::Key { pressed: true, .. })
        ));
        assert!(defer_maintenance(
            &receiver,
            &mut pending,
            Duration::from_millis(30),
            false
        ));
        assert!(matches!(pending, Some(Request::Key { pressed: false, .. })));
        assert!(!defer_maintenance(
            &receiver,
            &mut pending,
            Duration::from_millis(100),
            false
        ));
        assert!(matches!(
            pending.take(),
            Some(Request::Key { pressed: false, .. })
        ));
        assert!(!defer_maintenance(
            &receiver,
            &mut pending,
            Duration::ZERO,
            false
        ));
    }
    #[test]
    fn continuous_input_does_not_push_due_frames_to_the_100ms_metadata_deadline() {
        let (_sender, receiver) = mpsc::channel();
        let mut pending = Some(Request::Key {
            window_id: 1,
            keysym: 42,
            pressed: false,
        });
        let mut last_frame = Duration::ZERO;
        let mut last_scan = Duration::ZERO;
        let interval = Duration::from_micros(33_333);
        let mut frames = 0;
        for ms in 1..=1000 {
            let now = Duration::from_millis(ms);
            let frame_due = now - last_frame >= interval;
            if defer_maintenance(&receiver, &mut pending, now - last_scan, frame_due) {
                continue;
            }
            if now - last_scan >= Duration::from_millis(100) {
                last_scan = now;
            }
            if frame_due {
                frames += 1;
                last_frame = now;
            }
            assert!(
                matches!(pending, Some(Request::Key { pressed: false, .. })),
                "a due frame must preserve the queued key release"
            );
        }
        assert!(frames >= 29, "continuous input admitted only {frames} frames instead of the negotiated 30 FPS cadence");
    }
    fn motion(window_id: u32, x: i32) -> Request {
        Request::Pointer {
            window_id,
            x,
            y: 10,
            button: 0,
            pressed: false,
        }
    }
    #[test]
    fn obsolete_motion_is_replaced_without_crossing_input_or_window_boundaries() {
        for barrier in [
            Request::Key {
                window_id: 1,
                keysym: 42,
                pressed: true,
            },
            Request::Key {
                window_id: 1,
                keysym: 42,
                pressed: false,
            },
            Request::Pointer {
                window_id: 1,
                x: 8,
                y: 10,
                button: 1,
                pressed: true,
            },
            Request::Pointer {
                window_id: 1,
                x: 8,
                y: 10,
                button: 1,
                pressed: false,
            },
            Request::Pointer {
                window_id: 1,
                x: 8,
                y: 10,
                button: 4,
                pressed: true,
            },
            Request::Pointer {
                window_id: 1,
                x: 8,
                y: 10,
                button: 4,
                pressed: false,
            },
            Request::Text {
                window_id: 1,
                text: "text".into(),
            },
            Request::Paste {
                window_id: 1,
                text: "clipboard".into(),
            },
            motion(2, 8),
        ] {
            let (sender, receiver) = mpsc::channel();
            let expected = format!("{barrier:?}");
            sender.send(motion(1, 7)).unwrap();
            sender.send(motion(1, 8)).unwrap();
            sender.send(barrier).unwrap();
            sender.send(motion(1, 99)).unwrap();
            let mut pending = None;
            assert!(matches!(
                coalesce(motion(1, 1), &receiver, &mut pending),
                Request::Pointer { x: 8, .. }
            ));
            assert_eq!(format!("{:?}", pending.take().unwrap()), expected);
            assert!(matches!(
                receiver.try_recv().unwrap(),
                Request::Pointer { x: 99, .. }
            ));
        }
    }
    #[test]
    fn a_large_motion_burst_remains_bounded_and_preserves_the_final_release() {
        let (sender, receiver) = mpsc::channel();
        for x in 1..600 {
            sender.send(motion(1, x)).unwrap();
        }
        sender
            .send(Request::Key {
                window_id: 1,
                keysym: 42,
                pressed: false,
            })
            .unwrap();
        let mut pending = None;
        for (first, last) in [
            (motion(1, 0), 255),
            (motion(1, 256), 511),
            (motion(1, 512), 599),
        ] {
            assert!(
                matches!(coalesce(first, &receiver, &mut pending), Request::Pointer { x, .. } if x == last)
            );
            if last != 599 {
                assert!(pending.is_none());
                assert!(
                    matches!(receiver.try_recv().unwrap(), Request::Pointer { x, .. } if x == last + 1)
                );
            }
        }
        assert!(matches!(pending, Some(Request::Key { pressed: false, .. })));
    }
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
