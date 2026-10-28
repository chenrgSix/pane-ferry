import type { AppInfo, DesktopWindow } from "./launcher-types.ts";

export type AutomaticMode = "off" | "all" | "selected";
export type AutomaticRule = { mode: AutomaticMode; applications: string[] };
export type AutomaticPreference = { peerId: string; applicationKey: string; name: string; rule: AutomaticRule };
export type AutomaticStatus = { rules: AutomaticPreference[]; connectedApplications: string[]; warning: string };
export const emptyAutomaticStatus = (): AutomaticStatus => ({ rules: [], connectedApplications: [], warning: "" });

// Preference identities come from the publisher. Never use a document title
// or a PID/window ID as the persistent identity of a discovered application.
export function automaticApplications(apps: AppInfo[], windows: DesktopWindow[], saved: AutomaticPreference[], connected: string[] = []) {
  const options = new Map<string, { name: string; unavailableReason: string }>();
  for (const rule of saved) options.set(rule.applicationKey, { name: rule.name, unavailableReason: "当前未发现此应用，请刷新运行状态。" });
  for (const app of apps) options.set(app.id, { name: app.name, unavailableReason: app.unavailable_reason || "" });
  for (const window of windows) {
    const key = window.application_key || window.app_id;
    if (key) options.set(key, { name: options.get(key)?.name || window.name, unavailableReason: "" });
  }
  return [...options].map(([id, option]) => ({ id, ...option, connected: connected.includes(id),
    unavailableReason: connected.includes(id) ? "" : option.unavailableReason }))
    .sort((a, b) => Number(b.connected) - Number(a.connected) || Number(Boolean(a.unavailableReason)) - Number(Boolean(b.unavailableReason)) || a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
}
