#!/usr/bin/env python3
"""Opt-in context-menu latency on an explicitly authorized existing application.

Uses source geometry from normal attach. Only right-click and Escape are injected; a private
command file selects an observed safe location. No editor text or clipboard use.
"""
import argparse
import json
from pathlib import Path
import subprocess
from live_end_to_end import build

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--build-dir', type=Path, required=True)
parser.add_argument('--build-only', action='store_true')
for name in ('peer', 'state', 'helper', 'app', 'output', 'command-file'):
    parser.add_argument('--' + name)
args = parser.parse_args()
if args.build_only:
    print(build(args.build_dir.resolve(), Path(__file__).with_name('PopupProbe.swift')))
else:
    if any(getattr(args, name) is None for name in ('peer', 'state', 'helper', 'app', 'output', 'command_file')):
        parser.error('run requires peer, state, helper, app, output and command-file')
    bundle = args.build_dir.resolve() / 'PaneFerry Live Probe.app'
    subprocess.run(['open', '-W', '-n', '--env', 'PROBE_OUTPUT=' + str(Path(args.output).resolve()),
                    '--env', 'PROBE_COMMAND=' + str(Path(args.command_file).resolve()),
                    '--stdout', str(Path(args.output).with_suffix('.stdout').resolve()),
                    '--stderr', str(Path(args.output).with_suffix('.stderr').resolve()),
                    str(bundle), '--args', '--native-peer', args.peer, '--native-state', args.state,
                    '--native-helper', args.helper, '--app', args.app], check=True, timeout=240)
    result = json.loads(Path(args.output).read_text())
    assert 'error' not in result, result.get('error')
    assert len([r for r in result['frames'] if not r['warmup']]) == result['expected_samples']
    assert all(r['hardware_decoder'] and r['codec'] == 'h264' and r['borderless'] for r in result['frames'])
    assert all([r['width'], r['height']] == result['expected_popup_pixels'] and r['kind'] == 'menu'
               for r in result['frames'])
    print(json.dumps({'output': args.output, 'samples': result['expected_samples']}))
