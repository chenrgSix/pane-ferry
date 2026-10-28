import type { AppInfo, DesktopWindow } from "./launcher-types.ts";

export type AutomaticMode = "off" | "all" | "selected";
export type AutomaticRule = { mode: AutomaticMode; applications: string[] };
export type AutomaticPreference = { peerId: string; applicationKey: string; name: string; rule: AutomaticRule };
export type AutomaticStatus = { rules: AutomaticPreference[]; connectedApplications: string[]; warning: string };
export const emptyAutomaticStatus = (): AutomaticStatus => ({ rules: [], connectedApplications: [], warning: "" });

// Preference identities come from the publisher. Never use a document title
// or a PID/window ID as the persistent identity of a discovered application.
export function automaticApplications(apps: AppInfo[], windows: DesktopWindow[], saved: AutomaticPreference[]) {
  const options = new Map<string, string>();
  for (const rule of saved) options.set(rule.applicationKey, rule.name);
  for (const window of windows) {
    const key = window.application_key || window.app_id;
    if (key) options.set(key, window.name);
  }
  for (const app of apps) options.set(app.id, app.name);
  return [...options].map(([id, name]) => ({ id, name }))
    .sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
}
