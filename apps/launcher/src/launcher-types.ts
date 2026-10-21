export type Endpoint = { host: string; user: string; port: number };
export type LaunchTarget = { id: string; name: string; unavailable_reason?: string | null };
export type AppInfo = LaunchTarget & {
  description: string;
  icon_png?: string | null;
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
export type SessionInfo = {
  session_id: string;
  app_id: string;
  backend: "native";
  display: number;
  controlled: boolean;
};

export const endpointKey = (endpoint: Endpoint) => JSON.stringify({
  host: endpoint.host, user: endpoint.user, port: endpoint.port,
});
