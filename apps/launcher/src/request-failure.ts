const connectionKinds = new Set(["authentication_required", "connection", "host_verification", "agent_unavailable"]);

export function requestFailure(value: unknown): { message: string; kind: string } {
  if (value && typeof value === "object" && "message" in value && typeof value.message === "string") {
    const kind = "kind" in value && typeof value.kind === "string" ? value.kind : "unknown";
    return { message: value.message, kind };
  }
  return { message: String(value), kind: "unknown" };
}

export function isConnectionFailure(value: unknown): boolean {
  return connectionKinds.has(requestFailure(value).kind);
}
