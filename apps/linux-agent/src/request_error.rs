/// Stable public codes; never require a client to display arbitrary error details.
pub fn code(message: &str) -> &'static str {
    match message {
        "sharing disabled" => "sharing_disabled",
        "desktop_locked" => "desktop_locked",
        "resolution_limited" => "resolution_limited",
        "display_scale_requires_restart" => "display_scale_requires_restart",
        "display_settings_unavailable" => "display_settings_unavailable",
        "wayland_bridge_unavailable" => "wayland_bridge_unavailable",
        "wayland_bridge_failed" => "wayland_bridge_failed",
        "wayland_bridge_outdated" => "wayland_bridge_outdated",
        "wayland_capture_unavailable" => "wayland_capture_unavailable",
        "wayland_binary_clipboard_unavailable" => "wayland_binary_clipboard_unavailable",
        "wayland_cut_unavailable" => "wayland_cut_unavailable",
        "wayland_text_unavailable" => "wayland_text_unavailable",
        "wayland_text_outcome_unknown" => "wayland_text_outcome_unknown",
        "desktop_authorization_pending" => "desktop_authorization_pending",
        "desktop_authorization_denied" => "desktop_authorization_denied",
        "desktop_input_paused" => "desktop_input_paused",
        "desktop_input_unavailable" => "desktop_input_unavailable",
        "desktop_secrets_unavailable" => "desktop_secrets_unavailable",
        "application has multiple windows" => "application_multiple_windows",
        "application exited before creating a window" => "application_start_failed",
        "application window pending" => "application_window_pending",
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
