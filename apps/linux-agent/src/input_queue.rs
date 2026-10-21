use remoteapp_protocol::{Request, MAX_TEXT_BYTES};
use std::sync::mpsc::Receiver;

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
