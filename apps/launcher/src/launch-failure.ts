export function launchFailure(error: unknown): { message: string; retrySameRequest: boolean } {
  if (typeof error === "object" && error !== null && "message" in error &&
      typeof error.message === "string" && "retrySameRequest" in error &&
      typeof error.retrySameRequest === "boolean") {
    return { message: error.message, retrySameRequest: error.retrySameRequest };
  }
  // Unknown failures can include a lost successful launch reply.
  return { message: String(error), retrySameRequest: true };
}
