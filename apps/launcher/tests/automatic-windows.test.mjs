import assert from "node:assert/strict";
import { test } from "node:test";
import { automaticApplications } from "../src/automatic-windows.ts";

test("application choices merge catalog, uncatalogued service and saved rules without using window titles", () => {
  const apps = [{ id: "gpt", name: "ChatGPT" }];
  const windows = [
    { id: "window-1", app_id: "gpt", application_key: "gpt", name: "ChatGPT", title: "Private document" },
    { id: "window-2", application_key: "executable-service", name: "System chooser", title: "Select folder" },
    { id: "window-3", application_key: "executable-service", name: "System chooser", title: "Select another folder" },
    { id: "unidentified-window", name: "Unknown", title: "Not an application identity" },
  ];
  const saved = [{ applicationKey: "idea", name: "IDEA", rule: { mode: "all", applications: [] } }];
  const options = automaticApplications(apps, windows, saved);
  assert.equal(options.length, 3);
  assert.deepEqual(new Set(options.map(app => app.id)), new Set(["gpt", "idea", "executable-service"]));
  assert.ok(options.every(app => !app.name.includes("Private")));
});

test("stale custom entries remain editable but cannot masquerade as a connected application", () => {
  const options = automaticApplications([
    { id: "legacy", name: "ChatGPT（现有窗口）", unavailable_reason: "原进程已退出" },
    { id: "current", name: "ChatGPT" },
  ], [], [{ applicationKey: "legacy", name: "ChatGPT", rule: { mode: "all", applications: [] } }], ["current"]);
  assert.equal(options[0].id, "current");
  assert.equal(options[0].connected, true);
  assert.equal(options[0].unavailableReason, "");
  assert.equal(options[1].id, "legacy");
  assert.equal(options[1].connected, false);
  assert.equal(options[1].unavailableReason, "原进程已退出");
});

test("live window identities supersede stale catalog availability without merging equal names", () => {
  const saved = [{ applicationKey: "missing", name: "Same name" }];
  const options = automaticApplications([{ id: "live", name: "Same name", unavailable_reason: "stale" }],
    [{ id: "window", application_key: "live", name: "Same name", title: "Private title" }], saved);
  assert.equal(options[0].id, "live");
  assert.equal(options[0].unavailableReason, "");
  assert.equal(options[1].id, "missing");
  assert.ok(options[1].unavailableReason);
});
