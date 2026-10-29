#!/usr/bin/env python3
"""Launcher IPC → immutable app bundle → AppKit/Dock, using private publishers."""
from pathlib import Path
import argparse
import json
import os
import signal
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--host', required=True, type=Path)
parser.add_argument('--report', type=Path)
args = parser.parse_args()
root = Path(__file__).resolve().parents[2]
with tempfile.TemporaryDirectory(prefix='pf-packaged-group-', dir='/tmp') as directory:
    scratch = Path(directory)
    (scratch/'bridge').write_bytes((root/'tests/fixtures/group_publisher.py').read_bytes())
    (scratch/'bridge').chmod(0o700)
    (scratch/'video.json').write_bytes((root/'tests/fixtures/native-video.json').read_bytes())
    probe = scratch/'inspect.swift'
    probe.write_text(r'''
import AppKit
import ApplicationServices
let pid = pid_t(CommandLine.arguments[1])!
let bundleID = CommandLine.arguments[2]
let apps = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID)
precondition(apps.count == 1 && apps[0].processIdentifier == pid && apps[0].activationPolicy == .regular,
             "expected one foreground Mac application for the shared host")
func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var result: CFTypeRef?; AXUIElementCopyAttributeValue(element, name as CFString, &result); return result
}
var dockMatches = 0
if let dock = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.dock").first {
    let element = AXUIElementCreateApplication(dock.processIdentifier)
    AXUIElementSetMessagingTimeout(element, 0.2)
    var pending = [element], visited = 0
    while let current = pending.popLast(), visited < 2000 {
        visited += 1
        if (attribute(current, kAXTitleAttribute) as? String) == "PaneFerry 分组验收" { dockMatches += 1 }
        pending += attribute(current, kAXChildrenAttribute) as? [AXUIElement] ?? []
    }
}
let windows = (CGWindowListCopyWindowInfo(.optionAll, kCGNullWindowID) as? [[String: Any]] ?? []).filter {
    ($0[kCGWindowOwnerPID as String] as? Int32) == pid && ($0[kCGWindowLayer as String] as? Int) == 0 &&
    ($0[kCGWindowName as String] as? String ?? "").hasPrefix("Group fixture ")
}
precondition(windows.count == 2, "two session windows must belong to the same process")
if AXIsProcessTrusted() { precondition(dockMatches == 1, "expected exactly one Dock item; found \(dockMatches)") }
let result: [String: Any] = ["pid": pid, "bundleID": bundleID, "regularApplicationCount": apps.count,
    "windowCount": windows.count, "dockMatchingItems": dockMatches, "dockAccessibilityAvailable": AXIsProcessTrusted(),
    "windowTitles": windows.map { $0[kCGWindowName as String] as? String ?? "" }]
print(String(data: try! JSONSerialization.data(withJSONObject: result, options: .sortedKeys), encoding: .utf8)!)
''')
    probe_binary = scratch/'inspect'
    subprocess.run(['swiftc', '-module-cache-path', '/tmp/paneferry-appgroup-test-modules',
                    str(probe), '-o', str(probe_binary)], check=True, timeout=120)
    env = dict(os.environ, PF_GROUP_HOST=str(args.host.resolve()), PF_GROUP_FIXTURE=str(scratch),
               TAURI_CONFIG='{"bundle":{"resources":[]}}')
    log = scratch/'cargo.log'
    owner = None
    with log.open('w') as output:
        process = subprocess.Popen(['cargo', 'test', '--locked', '--release', '--manifest-path',
            str(root/'apps/launcher/src-tauri/Cargo.toml'),
            'host_group::tests::packaged_owner_reuses_application_and_detaches_sessions_independently',
            '--', '--exact', '--ignored', '--nocapture'], cwd=root, env=env, stdout=output, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic()+180
            while not (scratch/'owner.json').exists():
                assert process.poll() is None, log.read_text()[-6000:]
                assert time.monotonic() < deadline, log.read_text()[-6000:]
                time.sleep(.05)
            owner = json.loads((scratch/'owner.json').read_text())
            # Allow first video frames and WindowServer registration to settle.
            time.sleep(.5)
            checked = subprocess.run([str(probe_binary), str(owner['pid']), 'io.lanremoteapp.host.'+owner['group']],
                capture_output=True, text=True, timeout=10)
            assert checked.returncode == 0, checked.stderr[-4000:]
            report = json.loads(checked.stdout)
            (scratch/'inspection-complete').touch()
            assert process.wait(timeout=20) == 0, log.read_text()[-6000:]
            requests = [json.loads(line) for line in (scratch/'requests').read_text().splitlines()]
            for session in ('one', 'two', 'three'):
                assert len([r for r in requests if r['type'] == 'session.resume' and r.get('session_id') == session]) == 1
            report['perSessionResumeCounts'] = {s: 1 for s in ('one', 'two', 'three')}
            report['scopedDetachAndApplicationQuit'] = 'passed'
            if args.report:
                args.report.parent.mkdir(parents=True, exist_ok=True)
                args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2)+'\n')
            print('PASS:', json.dumps(report, ensure_ascii=False))
        finally:
            if owner:
                try:
                    os.kill(owner['pid'], signal.SIGTERM)
                except ProcessLookupError:
                    pass
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
