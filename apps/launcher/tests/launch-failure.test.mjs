import assert from "node:assert/strict";
import test from "node:test";
import { launchFailure } from "../src/launch-failure.ts";
import { PendingLaunches } from "../src/session-store.ts";

test("terminal session failure permits a new launch; lost replies keep the original ID", () => {
  const pending = new PendingLaunches();
  const id = pending.id("app");
  for (const error of ["connection failed", new Error("timeout"), { message: "unknown" }]) {
    assert.equal(launchFailure(error).retrySameRequest, true);
    assert.equal(pending.id("app"), id);
  }
  const failure = launchFailure({ message: "会话已结束", retrySameRequest: false });
  if (!failure.retrySameRequest) pending.resolve("app");
  assert.equal(failure.message, "会话已结束");
  assert.notEqual(pending.id("app"), id);
});
