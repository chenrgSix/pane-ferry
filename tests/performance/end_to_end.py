#!/usr/bin/env python3
"""Opt-in Mac -> SSH -> isolated Ubuntu GTK -> H.264 -> actual AppKit draw benchmark.

Builds temporary instrumented copies. Does not replace either installed app, load
the user's catalog, inspect the system clipboard or control existing sessions.
Clock differences are never subtracted: network/SSH is a round-trip residual.
"""
import argparse
import io
import json
import math
import os
from pathlib import Path
import plistlib
import re
import shlex
import shutil
import statistics
import subprocess
import tarfile
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent


def replace(source, before, after):
    assert source.count(before) == 1, f'Instrumentation anchor changed: {before[:100]}'
    return source.replace(before, after)


def instrument_agent(destination):
    main = destination / 'apps/linux-agent/src/main.rs'
    text = replace(main.read_text(), 'mod input_queue;', 'mod probe;\nmod input_queue;')
    text = replace(text, 'Ok(req) => {\n                            if tx.send(req)',
                   'Ok(req) => {\n                            probe::request("received", &req);\n                            if tx.send(req)')
    text = replace(text, 'let req = input_queue::coalesce(req, &rx, &mut pending_request);',
                   'let req = input_queue::coalesce(req, &rx, &mut pending_request);\n            probe::request("dispatched", &req);')
    text = replace(text, 'if let Ok(image) = b.capture(w) {',
                   'let probe_start = probe::now();\n                    if let Ok(image) = b.capture(w) {\n                        probe::capture(image.data.as_ptr() as usize, probe_start);')
    main.write_text(text)
    shutil.copy2(HERE / 'probe.rs', main.with_name('probe.rs'))
    output = main.with_name('output.rs')
    text = replace(output.read_text(), 'let mut value = json!({"type":"frame",',
                   'let probe = crate::probe::frame(image.data.as_ptr() as usize);\n                let mut value = json!({"type":"frame",')
    text = replace(text, '                frames.remove(&window.window_id);\n                if frames',
                   '                value["_probe"] = probe;\n                value["_probe"]["encode_end"] = json!(crate::probe::now());\n                frames.remove(&window.window_id);\n                if frames')
    text = replace(text, 'let mut bytes = serde_json::to_vec(&value)?;',
                   'if value.get("_probe").is_some() { value["_probe"]["ready"] = json!(crate::probe::now()); }\n        let mut bytes = serde_json::to_vec(&value)?;')
    output.write_text(text)


def instrument_mac(destination, bridge):
    original = ROOT / 'apps/mac-windowhost/Sources/RemoteAppWindowHost'
    shutil.copytree(original, destination)
    main = destination / 'main.swift'
    text = replace(main.read_text(), '".local/bin/remoteapp-agent bridge"]', json.dumps(bridge) + ']')
    text = replace(text, 'let line = buffer.prefix(upTo: newline)',
                   'let probeReceived = Probe.now()\n                    let line = buffer.prefix(upTo: newline)')
    text = replace(text, 'try enqueue(Incoming(object: lightweight, image: image))',
                   'lightweight["received"] = probeReceived\n                    lightweight["decoded"] = Probe.now()\n                    lightweight["wire_bytes"] = length + 1\n                    if let id = unsignedInteger(object["window_id"]) { lightweight["hardware_decoder"] = videoDecoders[UInt32(id)]?.hardwareAccelerated ?? false }\n                    try enqueue(Incoming(object: lightweight, image: image))')
    text = replace(text, 'do { try input.fileHandleForWriting.write(contentsOf: next) }',
                   'do { let start = Probe.now(); try input.fileHandleForWriting.write(contentsOf: next); Probe.written(next, start: start, end: Probe.now()) }')
    text = replace(text, 'host.surface.image = image',
                   'Probe.selected(image, object: object)\n            host.surface.image = image')
    text = replace(text, 'DispatchQueue.main.async { [weak host] in host?.fitToScreen() }',
                   'DispatchQueue.main.async { [weak host] in if let host = host { Probe.configure(host) } }')
    text = replace(text, 'application.delegate = applicationDelegate\n    application.run()',
                   'application.delegate = applicationDelegate\n    Probe.start(applicationDelegate!)\n    application.run()')
    text = replace(text, '        let alert = NSAlert(); alert.messageText = "LAN RemoteApp"; alert.informativeText = message\n        alert.addButton(withTitle: "好"); alert.runModal()',
                   '        Probe.finish(error: message)')
    text = replace(text, 'func send(_ object: [String: Any]) {',
                   'func send(_ object: [String: Any]) {\n        if ["hello", "video.enable", "app.launch"].contains(object["type"] as? String ?? "") { FileHandle.standardError.write(Data(("Probe sends " + (object["type"] as? String ?? "") + "\\n").utf8)) }')
    text = replace(text, 'private func enqueue(_ value: Incoming) throws {',
                   'private func enqueue(_ value: Incoming) throws {\n        if ["hello", "video.ready", "session.started", "error"].contains(value.object["type"] as? String ?? "") { FileHandle.standardError.write(Data(("Probe receives " + (value.object["type"] as? String ?? "") + "\\n").utf8)) }')
    text = replace(text, 'if type == "session.exited" {',
                   'if type == "session.exited" {\n            Probe.finish(error: "test fixture exited before measurement completed")')
    main.write_text(text)
    surface = destination / 'SurfaceView.swift'
    text = replace(surface.read_text(), '        if hasMarkedText() {\n',
                   '        Probe.drew(image)\n        if hasMarkedText() {\n')
    # Prevent a click/key in another foreground task from contaminating the
    # benchmark if the test window happens to appear under the user's pointer.
    for signature in ('override func mouseDown(with event: NSEvent) {',
                      'override func keyDown(with event: NSEvent) {',
                      'func insertText(_ string: Any, replacementRange: NSRange) {'):
        text = replace(text, signature, signature + '\n        guard Probe.injecting else { return }')
    surface.write_text(text)
    shutil.copy2(HERE / 'Probe.swift', destination / 'Probe.swift')


def summary(values):
    ordered = sorted(values)
    return dict(mean=round(statistics.mean(values), 2), median=round(statistics.median(values), 2),
                p95=round(ordered[math.ceil(.95 * len(ordered)) - 1], 2),
                min=round(min(values), 2), max=round(max(values), 2))


def analyze(mac, remote):
    assert 'error' not in mac, mac.get('error')
    received = {x['count']: x['ns'] for x in remote['agent'] if x['kind'] == 'received'}
    dispatched = {x['count']: x['ns'] for x in remote['agent'] if x['kind'] == 'dispatched'}
    fixture = {x['count']: x for x in remote['fixture'] if x['kind'] != 'draw'}
    draws = {x['count']: x['ns'] for x in remote['fixture'] if x['kind'] == 'draw'}
    rows = []
    for frame in mac['frames']:
        count = frame['count']
        assert fixture[count]['kind'] == frame['mode'], (count, fixture[count], frame['mode'])
        assert frame['codec'] == 'h264' and frame['hardware_decoder'], 'video/hardware decoder fallback'
        if frame['warmup']:
            continue
        p = frame['_probe']
        app = fixture[count]['ns']
        server = p['ready'] - received[count]
        row = dict(count=count, mode=frame['mode'], bytes=frame['wire_bytes'], key_frame=frame['key_frame'],
                   total=frame['draw_end'] - frame['start'],
                   controller_input=frame['write_end'] - frame['start'],
                   agent_input_queue=dispatched[count] - received[count],
                   x11_delivery=app - dispatched[count],
                   app_render=draws[count] - app,
                   capture_wait=max(0, p['capture_start'] - draws[count]),
                   capture=p['capture_end'] - p['capture_start'],
                   response_to_capture=p['capture_end'] - app,
                   encoder_queue=p['encode_start'] - p['capture_end'],
                   encode=p['encode_end'] - p['encode_start'],
                   server_prepare=p['ready'] - p['encode_end'],
                   ssh_network=(frame['received'] - frame['write_end']) - server,
                   decode=frame['decoded'] - frame['received'],
                   main_queue=frame['selected'] - frame['decoded'],
                   draw=frame['draw_end'] - frame['selected'])
        for key in row.keys() - {'count', 'mode', 'bytes', 'key_frame'}:
            row[key] /= 1_000_000
        # Additive stages close exactly; app_render/capture_wait/capture are
        # supplementary diagnostics and can overlap a blocking X11 GetImage.
        closed = sum(row[key] for key in ('controller_input', 'agent_input_queue', 'x11_delivery',
                     'response_to_capture', 'encoder_queue', 'encode', 'server_prepare',
                     'ssh_network', 'decode', 'main_queue', 'draw'))
        assert abs(closed - row['total']) < .001
        rows.append(row)
    metrics = {key: summary([row[key] for row in rows])
               for key in rows[0].keys() - {'count', 'mode', 'bytes', 'key_frame'}}
    modes = {mode: summary([row['total'] for row in rows if row['mode'] == mode])
             for mode in ('key', 'text', 'pointer')}
    groups = dict(input=('controller_input', 'agent_input_queue', 'x11_delivery'),
                  remote_capture=('response_to_capture',),
                  remote_encode=('encoder_queue', 'encode', 'server_prepare'),
                  transport=('ssh_network',), mac=('decode', 'main_queue', 'draw'))
    grouped = {name: summary([sum(row[key] for key in keys) for row in rows]) for name, keys in groups.items()}
    return dict(samples=len(rows), stages_ms=metrics, modes_ms=modes, groups_ms=grouped,
                response_packet_bytes=summary([row['bytes'] for row in rows]), rows=rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--host', required=True)
    parser.add_argument('--user', required=True)
    parser.add_argument('--port', type=int, default=22)
    parser.add_argument('--width', type=int, default=2560)
    parser.add_argument('--height', type=int, default=1664)
    parser.add_argument('--samples', type=int, default=30, help='measured samples per input kind, plus two warmups')
    parser.add_argument('--animated', action='store_true')
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--remote-build-cache', help='existing task-owned Cargo target directory')
    parser.add_argument('--mac-module-cache', type=Path, help='task-owned Swift module cache reused across runs')
    args = parser.parse_args()
    assert re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9.-]{0,252}', args.host)
    assert re.fullmatch(r'[A-Za-z0-9_][A-Za-z0-9_-]{0,63}', args.user)
    assert 0 < args.port < 65536 and 1 <= args.samples <= 70
    assert 320 <= args.width <= 4096 and 128 <= args.height <= 2160 and args.width * args.height <= 8_000_000
    args.output.parent.mkdir(parents=True, exist_ok=True)
    ssh = ['/usr/bin/ssh', '-F', '/dev/null', '-S', '~/.ssh/lan-remoteapp-%C', '-T',
           '-oBatchMode=yes', '-oStrictHostKeyChecking=yes', '-oConnectTimeout=8',
           '-oForwardAgent=no', '-oClearAllForwardings=yes', '-p', str(args.port), '-l', args.user, '--', args.host]

    def remote_run(argv, **kwargs):
        return subprocess.run(ssh + [shlex.join(argv)], check=True, **kwargs)

    remote_dir = remote_run(['mktemp', '-d', '/tmp/remoteapp-end-to-end-XXXXXX'],
                            capture_output=True, text=True, timeout=15).stdout.strip()
    assert re.fullmatch('/tmp/remoteapp-end-to-end-[A-Za-z0-9]+', remote_dir)
    manager = None
    try:
        with tempfile.TemporaryDirectory(prefix='remoteapp-end-to-end-', dir='/tmp') as temporary:
            scratch = Path(temporary)
            agent_source = scratch / 'agent-source'
            for name in ('Cargo.toml', 'Cargo.lock'):
                agent_source.mkdir(exist_ok=True)
                shutil.copy2(ROOT / name, agent_source / name)
            shutil.copytree(ROOT / 'crates', agent_source / 'crates', ignore=shutil.ignore_patterns('target'))
            for name in ('linux-agent',):
                shutil.copytree(ROOT / 'apps' / name, agent_source / 'apps' / name,
                                ignore=shutil.ignore_patterns('target', '.build'))
            # Cargo's workspace excludes this app, but all remaining members must exist.
            instrument_agent(agent_source)
            shutil.copytree(HERE, agent_source / 'tests/performance', ignore=shutil.ignore_patterns('__pycache__'))
            archive = io.BytesIO()
            with tarfile.open(fileobj=archive, mode='w:gz', format=tarfile.PAX_FORMAT) as tar:
                tar.add(agent_source, arcname='source')
            print('Building isolated Ubuntu timing probe...', flush=True)
            remote_run(['tar', 'xzf', '-', '-C', remote_dir], input=archive.getvalue(), timeout=30)
            cargo_target = args.remote_build_cache or remote_dir + '/target'
            remote_run(['/usr/bin/env', 'CARGO_TARGET_DIR=' + cargo_target, 'cargo', 'build', '--release',
                        '--locked', '--offline', '--manifest-path', remote_dir + '/source/Cargo.toml',
                        '-p', 'remoteapp-agent'], stdout=subprocess.DEVNULL, timeout=180)
            agent = cargo_target + '/release/remoteapp-agent'
            home = remote_dir + '/home'
            command = ['/usr/bin/python3', remote_dir + '/source/tests/performance/remote.py',
                       '--agent', agent, '--home', home, '--width', str(args.width), '--height', str(args.height)]
            if args.animated:
                command.append('--animated')
            manager = subprocess.Popen(ssh + [shlex.join(command)], stdin=subprocess.PIPE,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            ready = json.loads(manager.stdout.readline())
            assert ready['ready'], ready
            bridge = shlex.join(['/usr/bin/env', 'HOME=' + home, agent, 'bridge'])
            mac_source = scratch / 'mac-source'
            instrument_mac(mac_source, bridge)
            bundle = scratch / 'RemoteAppLatencyProbe.app'
            (bundle / 'Contents/MacOS').mkdir(parents=True)
            binary = bundle / 'Contents/MacOS/RemoteAppLatencyProbe'
            (bundle / 'Contents/Info.plist').write_bytes(plistlib.dumps(dict(
                CFBundleIdentifier='local.lanremoteapp.latency-probe', CFBundleName='RemoteApp Latency Probe',
                CFBundleExecutable='RemoteAppLatencyProbe', CFBundlePackageType='APPL',
                NSHighResolutionCapable=True, LSMinimumSystemVersion='12.0')))
            print('Building actual AppKit host with timing hooks...', flush=True)
            modules = args.mac_module_cache or scratch / 'modules'
            subprocess.run(['swiftc', '-O', '-whole-module-optimization', '-module-cache-path', str(modules),
                            *map(str, sorted(mac_source.glob('*.swift'))), '-o', str(binary)], check=True, timeout=120)
            env = dict(os.environ, PROBE_WIDTH=str(args.width), PROBE_HEIGHT=str(args.height),
                       PROBE_SAMPLES=str(args.samples), PROBE_OUTPUT=str(scratch / 'mac.json'))
            print(f'Measuring {args.samples * 3} steady-state input responses at {args.width}x{args.height}...', flush=True)
            run = subprocess.run([str(binary), '--host', args.host, '--user', args.user, '--port', str(args.port),
                                  '--app', 'latency'], env=env, timeout=100)
            mac = json.loads((scratch / 'mac.json').read_text())
            stdout, stderr = manager.communicate('stop\n', timeout=20)
            assert manager.returncode == 0, stderr
            manager = None
            remote = json.loads(stdout)
            if run.returncode:
                args.output.write_text(json.dumps(dict(mac=mac, remote=remote), indent=2) + '\n')
                raise AssertionError(mac.get('error', 'test host failed'))
            report = dict(host=args.host, width=args.width, height=args.height,
                          animated=args.animated, measured_at=time.strftime('%Y-%m-%dT%H:%M:%S%z'),
                          endpoint=mac['endpoint'], injection=mac['injection'],
                          **analyze(mac, remote), raw=dict(mac=mac, remote=remote))
            args.output.write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps({key: value for key, value in report.items() if key not in ('raw', 'rows')}, indent=2))
    finally:
        if manager is not None:
            try:
                manager.communicate('stop\n', timeout=15)
            except subprocess.TimeoutExpired:
                manager.kill()
                manager.wait(timeout=5)
        remote_run(['rm', '-rf', '--', remote_dir], stdout=subprocess.DEVNULL, timeout=15)


if __name__ == '__main__':
    main()
