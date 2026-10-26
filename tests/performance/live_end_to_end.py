#!/usr/bin/env python3
"""Build an opt-in WindowHost probe for an already paired, installed Agent.

Only --app's test-owned window is controlled. No installed binary is changed.
Start wayland_latency_fixture.py on the publisher, then use its window.list ID.
"""
import argparse
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def replace(source, before, after):
    if source.count(before) != 1:
        raise RuntimeError(f'Instrumentation anchor changed: {before[:100]}')
    return source.replace(before, after)


def build(destination, probe_source=HERE / 'LiveProbe.swift'):
    source = destination / 'Sources'
    shutil.copytree(ROOT / 'apps/mac-windowhost/Sources/PaneFerryWindowHost', source,
                    dirs_exist_ok=True)
    main = source / 'main.swift'
    text = main.read_text()
    replacements = [
        ('let line = buffer.prefix(upTo: newline)',
         'let probeReceived = Probe.now()\n                    let line = buffer.prefix(upTo: newline)'),
        ('lightweight["received_ns"] = DispatchTime.now().uptimeNanoseconds',
         'lightweight["received_ns"] = probeReceived; lightweight["wire_bytes"] = length + 1'),
        ('lightweight["hardware_decoder"] = hardware',
         'lightweight["hardware_decoder"] = hardware; lightweight["received"] = job.object["received_ns"]; lightweight["decoded"] = lightweight["decode_completed_ns"]'),
        ('do { try input.fileHandleForWriting.write(contentsOf: next) }',
         'do { let start = Probe.now(); try input.fileHandleForWriting.write(contentsOf: next); Probe.written(next, start: start, end: Probe.now()) }'),
        ('host.present(image, mediaMode: "H.264 视频")',
         'Probe.selected(image, object: object)\n            host.present(image, mediaMode: "H.264 视频")'),
        ('DispatchQueue.main.async { [weak host] in host?.fitToScreen() }',
         'DispatchQueue.main.async { [weak host] in if let host = host { Probe.configure(host) } }'),
        ('application.delegate = applicationDelegate\n    application.run()',
         'application.delegate = applicationDelegate\n    Probe.start(applicationDelegate!)\n    application.run()'),
        ('        let alert = NSAlert(); alert.messageText = applicationName; alert.informativeText = message\n        if let icon = applicationIcon { alert.icon = icon }\n        alert.addButton(withTitle: "好"); alert.runModal()',
         '        Probe.finish(error: message)'),
        ('if type == "session.exited" {',
         'if type == "session.exited" {\n            Probe.finish(error: "test fixture exited")'),
    ]
    for before, after in replacements:
        text = replace(text, before, after)
    if probe_source.name == 'LiveProbe.swift':
        # Benchmark-only override: reproduce 2x with the current publisher,
        # keeping every other product path identical to the 1x measurement.
        text = replace(text, 'launch["scale_factor"] = 1', 'launch["scale_factor"] = Probe.scale')
        text = replace(text, 'if type == "video.ready" {',
                       'if type == "video.ready" { Probe.negotiatedFPS = object["max_fps"] as? Int;')
    main.write_text(text)
    surface = source / 'SurfaceView.swift'
    if (source / 'VideoFrame.swift').exists():
        text = replace(surface.read_text(), '        guard displayFrame === frame else { return }',
                       '        guard displayFrame === frame else { return }\n        Probe.presented(frame, surface: self)')
        text = replace(text, 'CATransaction.setCompletionBlock { [weak self, weak frame] in',
                       'CATransaction.setCompletionBlock { [weak self, weak frame] in\n            if let frame = frame { Probe.videoStage(frame, "transaction_completion") }')
        text = replace(text, '                self.frameWasPresented(frame)',
                       '                Probe.videoStage(frame, "completion_on_main")\n                self.frameWasPresented(frame)')
        text = replace(text, '            renderer.enqueue(sample)',
                       '            Probe.videoStage(frame, "enqueue_begin")\n            renderer.enqueue(sample)\n            Probe.videoStage(frame, "enqueue_end")')
        text = replace(text, '            canvas.display.enqueue(sample)',
                       '            Probe.videoStage(frame, "enqueue_begin")\n            canvas.display.enqueue(sample)\n            Probe.videoStage(frame, "enqueue_end")')
        text = replace(text, '        CATransaction.commit()\n    }\n    override func layout()',
                       '        Probe.videoStage(frame, "commit_begin")\n        CATransaction.commit()\n        Probe.videoStage(frame, "commit_return")\n    }\n    override func layout()')
    else:
        text = replace(surface.read_text(), '        if hasMarkedText() {\n',
                       '        Probe.drew(image)\n        if hasMarkedText() {\n')
    for signature in ('override func mouseDown(with event: NSEvent) {',
                      'override func keyDown(with event: NSEvent) {',
                      'func insertText(_ string: Any, replacementRange: NSRange) {'):
        text = replace(text, signature, signature + '\n        guard Probe.injecting else { return }')
    if probe_source.name == 'PopupProbe.swift':
        for signature in ('override func rightMouseDown(with event: NSEvent) {',
                          'override func otherMouseDown(with event: NSEvent) {',
                          'override func mouseMoved(with event: NSEvent) {',
                          'override func scrollWheel(with event: NSEvent) {'):
            text = replace(text, signature, signature + '\n        guard Probe.injecting else { return }; ')
    surface.write_text(text)
    probe = probe_source.read_text()
    if (source / 'VideoFrame.swift').exists():
        probe = probe.replace('context menu AppKit draw completion', 'context menu renderer displayed-buffer availability')
        probe = probe.replace('"AppKit draw completion"', '"renderer displayed-buffer availability"')
        if probe_source.name == 'PopupProbe.swift':
            probe = replace(probe, '        if sceneMode {\n            result["source_geometry"]',
                '        result["video_counts"] = ["selected": selectedVideoCount, "presented": presentedVideoCount, "matched": matchedVideoCount]\n        if sceneMode {\n            result["source_geometry"]')
        probe += (HERE / 'VideoPresentationProbe.swift').read_text()
    (source / 'Probe.swift').write_text(probe)
    contents = destination / 'PaneFerry Live Probe.app/Contents'
    binary = contents / 'MacOS/PaneFerryLiveProbe'
    binary.parent.mkdir(parents=True, exist_ok=True)
    (contents / 'Info.plist').write_bytes(plistlib.dumps({
        'CFBundleIdentifier': 'io.paneferry.live-latency-probe',
        'CFBundleName': 'PaneFerry Live Probe', 'CFBundleExecutable': binary.name,
        'CFBundlePackageType': 'APPL', 'NSHighResolutionCapable': True,
        'LSMinimumSystemVersion': '12.0',
    }))
    subprocess.run(['swiftc', '-O', '-whole-module-optimization', '-module-cache-path',
                    str(destination / 'modules'), *map(str, sorted(source.glob('*.swift'))),
                    '-o', str(binary)], check=True)
    return binary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--build-dir', type=Path, required=True)
    parser.add_argument('--build-only', action='store_true')
    for name in ('peer', 'state', 'helper', 'app', 'output'):
        parser.add_argument('--' + name)
    parser.add_argument('--width', type=int, default=1470)
    parser.add_argument('--height', type=int, default=809)
    parser.add_argument('--scale', type=int, choices=(1, 2), default=1)
    parser.add_argument('--fps', type=int, choices=(30, 60), default=60)
    parser.add_argument('--samples', type=int, default=40)
    args = parser.parse_args()
    if not 1 <= args.samples <= 120:
        parser.error('--samples must be 1..120 (8-bit visual marker)')
    if args.build_only:
        print(build(args.build_dir.resolve()))
        return
    if any(getattr(args, name) is None for name in ('peer', 'state', 'helper', 'app', 'output')):
        parser.error('run requires --peer --state --helper --app --output')
    binary = args.build_dir.resolve() / 'PaneFerry Live Probe.app/Contents/MacOS/PaneFerryLiveProbe'
    env = dict(os.environ, PROBE_WIDTH=str(args.width), PROBE_HEIGHT=str(args.height),
               PROBE_SCALE=str(args.scale), PROBE_SAMPLES=str(args.samples),
               PROBE_OUTPUT=str(Path(args.output).resolve()))
    subprocess.run([str(binary), '--native-peer', args.peer, '--native-state', args.state,
                    '--native-helper', args.helper, '--app', args.app, '--fps', str(args.fps)], env=env, check=True,
                   timeout=100)
    result = json.loads(Path(args.output).read_text())
    rows = [row for row in result['frames'] if not row['warmup']]
    assert len(rows) == args.samples * 2 and 'error' not in result, result.get('error')
    assert all(row['codec'] == 'h264' and row['hardware_decoder'] for row in rows)
    assert result['scale'] == args.scale and result['negotiated_fps'] == args.fps, result
    assert result['endpoint'] == 'renderer displayed-buffer availability', result['endpoint']
    print(json.dumps({'output': args.output, 'samples': len(rows), 'hardware_decoder': True}))


if __name__ == '__main__':
    main()
