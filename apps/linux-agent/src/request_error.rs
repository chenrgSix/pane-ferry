/// Stable public codes; never require a client to display arbitrary error details.
pub fn code(message: &str) -> &'static str {
    match message {
        "desktop_authorization_pending" => "desktop_authorization_pending",
        "desktop_authorization_denied" => "desktop_authorization_denied",
        "desktop_input_paused" => "desktop_input_paused",
        "desktop_input_unavailable" => "desktop_input_unavailable",
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
        "terminal clipboard cut unavailable" => "clipboard_cut_unavailable",
        "app not approved" => "app_not_approved",
        "desktop application unavailable" => "desktop_application_unavailable",
        "application discovery unavailable" => "application_discovery_unavailable",
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
        assert_eq!(
            code("terminal clipboard cut unavailable"),
            "clipboard_cut_unavailable"
        );
        assert_eq!(code("unanticipated private details"), "request_failed");
    }
}
