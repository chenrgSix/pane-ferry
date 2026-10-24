import type { AppInfo, DesktopWindow, SessionInfo } from "./launcher-types";

export type Connection = {
  key: string;
  appId: string;
  sessionId: string | null;
  name: string;
  title: string;
  icon_png?: string | null;
  kind: "desktop" | "remote";
  minimized: boolean;
  controlled: boolean;
};

// Windows and session metadata arrive independently. Match by session ID first,
// then by the stable desktop-window ID, without collapsing independent instances.
export function connections(
  apps: AppInfo[], windows: DesktopWindow[], sessions: SessionInfo[],
): Connection[] {
  const rows = new Map<string, Connection>();
  for (const session of sessions) {
    if (session.mode === "desktop") continue;
    // An application can remain alive in the tray after its last window closes.
    // Keep it out of the window list until a visible window returns.
    if (session.window_count === 0 || session.unavailable_reason) continue;
    const window = windows.find(w => w.session_id === session.session_id)
      || windows.find(w => !w.session_id && w.id === session.app_id);
    // Session metadata can outlive the desktop window inventory during an
    // X11 withdraw/close. Do not offer a stale desktop session as a window.
    if (session.display === 0 && !window) continue;
    const app = apps.find(a => a.id === session.app_id);
    rows.set(`session:${session.session_id}`, {
      key: `session:${session.session_id}`, appId: session.app_id, sessionId: session.session_id,
      name: window?.name || app?.name || session.app_id,
      title: window?.title || (session.display === 0 ? "已接管的桌面窗口" : "远程应用窗口"),
      icon_png: window?.icon_png || app?.icon_png,
      kind: window || session.display === 0 ? "desktop" : "remote",
      minimized: window?.minimized ?? false,
      controlled: session.controlled || Boolean(window?.controlled),
    });
  }
  for (const window of windows) {
    const sessionId = window.session_id || sessions.find(s => s.app_id === window.id)?.session_id || null;
    const key = sessionId ? `session:${sessionId}` : `window:${window.id}`;
    const existing = rows.get(key);
    if (existing) {
      existing.controlled ||= window.controlled;
      continue;
    }
    rows.set(key, {
      key, appId: window.id, sessionId, name: window.name, title: window.title || "无标题窗口",
      icon_png: window.icon_png, kind: "desktop", minimized: window.minimized, controlled: window.controlled,
    });
  }
  return [...rows.values()].sort((a, b) => a.name.localeCompare(b.name) || a.title.localeCompare(b.title) || a.key.localeCompare(b.key));
}
