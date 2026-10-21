/// Stable public codes; never require a client to display arbitrary error details.
pub fn code(message: &str) -> &'static str {
    match message {
        "window is hidden" | "unknown window" => "window_unavailable",
        "approved existing process unavailable" => "existing_process_unavailable",
        "existing process does not match approved executable" => "existing_process_mismatch",
        "no X11 windows belong to the approved process" => "existing_window_unavailable",
        "session expired" | "unknown session" => "session_expired",
        "session exited" | "session exited; use a new request id to start again" => {
            "session_exited"
        }
        "session already controlled" => "session_controlled",
        "request id conflict" => "request_id_conflict",
        "app not approved" => "app_not_approved",
        "active session limit reached" => "session_limit",
        "request history limit reached; restart after saving work" => "request_history_limit",
        _ => "request_failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinguishes_stale_targets_and_terminal_requests() {
        assert_eq!(
            code("approved existing process unavailable"),
            "existing_process_unavailable"
        );
        assert_eq!(
            code("session exited; use a new request id to start again"),
            "session_exited"
        );
        assert_eq!(code("session expired"), "session_expired");
        assert_eq!(code("session already controlled"), "session_controlled");
        assert_eq!(code("unknown window"), "window_unavailable");
        assert_eq!(code("unanticipated private details"), "request_failed");
    }
}
