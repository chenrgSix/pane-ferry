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
