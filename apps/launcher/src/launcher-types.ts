export type Endpoint = { peerId: string };
export const validPeerId = (id: unknown): id is string => typeof id === "string" && /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(id);
export type LaunchTarget = { id: string; name: string; icon_png?: string | null; unavailable_reason?: string | null };
export type AppInfo = LaunchTarget & {
  description: string;
  source: "desktop" | "custom";
};
export type AppCatalog = { apps: AppInfo[]; discovery_error: string | null };
export type DesktopWindow = LaunchTarget & {
  window_id: number;
  title: string;
  icon_png?: string | null;
  minimized: boolean;
  controlled: boolean;
  session_id?: string | null;
};
export type DesktopWindows = { windows: DesktopWindow[]; unavailable_reason: string | null };
export type DesktopMonitor = { id: string; name: string; width: number; height: number; primary: boolean };
export type DesktopMonitors = { monitors: DesktopMonitor[]; unavailable_reason: string | null };
export type SessionInfo = {
  session_id: string;
  app_id: string;
  backend: "native";
  display: number;
  controlled: boolean;
  mode?: "app" | "desktop" | null;
  monitor_id?: string | null;
  window_count?: number | null;
  unavailable_reason?: string | null;
};

export const endpointKey = (endpoint: Endpoint) => JSON.stringify({transport:"quic", deviceId:endpoint.peerId});
