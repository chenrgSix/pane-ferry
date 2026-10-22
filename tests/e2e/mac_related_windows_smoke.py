#!/usr/bin/env python3
"""Opt-in real SCK/AppKit test. Controls only the dedicated fixture's windows.

Requires existing screen-recording/accessibility authorization. --hold keeps
the isolated publisher/fixture alive for a separate real QUIC receiver test.
"""
import argparse
import base64
import json
import os
from pathlib import Path
import plistlib
import signal
import socket
import subprocess
import tempfile
import time

root = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--directory', type=Path)
parser.add_argument('--hold', action='store_true')
args = parser.parse_args()
temporary = None if args.directory else tempfile.TemporaryDirectory(prefix='lra-related-', dir='/tmp')
scratch = (args.directory or Path(temporary.name)).resolve()
scratch.mkdir(mode=0o700, parents=True, exist_ok=True)
children = []


def wait(predicate, description, timeout=20):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        value = predicate()
        if value: return value
        time.sleep(.05)
    raise AssertionError(description)


class Peer:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX); self.sock.settimeout(15); self.sock.connect(str(path))
        self.reader = self.sock.makefile('rb')
        self.send('hello'); self.until('hello')
    def send(self, kind, **fields):
        self.sock.sendall((json.dumps(dict(type=kind, version=1, **fields))+'\n').encode())
    def until(self, kind, window=None):
        for _ in range(512):
            line = self.reader.readline(16*1024*1024+1)
            assert line.endswith(b'\n') and len(line)<=16*1024*1024, 'truncated protocol'
            value = json.loads(line)
            assert value['type']!='error' or kind=='error', value
            if value['type']==kind and (window is None or value.get('window_id')==window): return value
        raise AssertionError('missing '+kind)
    def request(self, kind, expected=None, **fields):
        self.send(kind, **fields); return self.until(expected or kind)
    def close(self):
        try: self.sock.shutdown(socket.SHUT_RDWR)
        except OSError: pass
        self.reader.close(); self.sock.close()


def bundle(path, identifier, executable, accessory=False):
    (path/'Contents/MacOS').mkdir(parents=True, exist_ok=True)
    (path/'Contents/Info.plist').write_bytes(plistlib.dumps(dict(CFBundleIdentifier=identifier,
        CFBundleName='RemoteApp Related Test', CFBundleDisplayName='RemoteApp Related Test',
        CFBundleExecutable=executable, CFBundlePackageType='APPL', LSUIElement=accessory,
        NSHighResolutionCapable=True)))
    return path/'Contents/MacOS'/executable


def start(command, name):
    with (scratch/(name+'.log')).open('w') as log:
        process = subprocess.Popen(command, stdout=log, stderr=log, start_new_session=True)
    children.append(process)
    return process


def stop_helpers():
    # Foundation.Process can create a distinct process group. Clean up only
    # this test directory's exact helper executable, including after a failure.
    executable=str(scratch/'Fixture.app/Contents/Helpers/Preview.app/Contents/MacOS/Preview')
    for line in subprocess.check_output(['ps','-axo','pid=,command='],text=True).splitlines():
        fields=line.strip().split(None,1)
        if len(fields)==2 and fields[1].startswith(('/tmp/','/private/tmp/')) and Path(fields[1].split(' ',1)[0]).resolve()==Path(executable):
            try:os.kill(int(fields[0]),signal.SIGTERM)
            except ProcessLookupError:pass


peers = []
try:
    stop_helpers()
    for name in ('fixture.json','fixture.json.preview','ready.json','stop'):
        (scratch/name).unlink(missing_ok=True)
    app = scratch/'Fixture.app'
    executable = bundle(app, 'dev.lra.related.fixture', 'Fixture')
    helper = bundle(app/'Contents/Helpers/Preview.app', 'dev.lra.related.fixture.helper', 'Preview', True)
    source = root/'apps/mac-node/Sources/RemoteAppNode'
    node = bundle(scratch/'Node.app', 'dev.lra.related.node', 'Node', True)
    cache = scratch/'modules'
    subprocess.run(['swiftc', '-parse-as-library', '-module-cache-path', str(cache), str(root/'tests/fixtures/MacRelatedWindowFixture.swift'), '-o', str(executable)], check=True, timeout=120)
    helper.write_bytes(executable.read_bytes()); helper.chmod(0o700)
    subprocess.run(['swiftc', '-module-cache-path', str(cache),
        *[str(path) for path in sorted(source.glob('*.swift')) if path.name!='main.swift'],
        str(root/'tests/fixtures/MacNodeHarness.swift'), '-o', str(node)], check=True, timeout=120)
    directory = scratch/'n'; directory.mkdir(exist_ok=True, mode=0o700)
    (directory/'sharing.json').write_text('{"enabled":true}')
    state = scratch/'fixture.json'
    fixture = start([str(executable), str(state)], 'fixture')
    service = start([str(node), str(directory)], 'publisher')
    wait(lambda: (directory/'node.sock').exists() and state.exists(), 'fixture and publisher ready')
    inventory = Peer(directory/'node.sock'); peers.append(inventory)
    info = inventory.request('node.info')
    assert info['permissions']['screen_recording'] and info['permissions']['accessibility'], 'existing recording/accessibility permissions required for test publisher'
    initial = json.loads(state.read_text())
    ids = [w['id'] for w in initial['windows']]; assert len(ids)==2
    inventory.request('app.list')
    targets = inventory.request('window.list')['windows']
    alpha = next(w for w in targets if w['window_id']==ids[0])
    beta = next(w for w in targets if w['window_id']==ids[1])
    selected = Peer(directory/'node.sock'); peers.append(selected)
    selected.request('video.enable', 'video.ready', codec='h264')
    sid = selected.request('app.launch', 'session.started', app_id=alpha['id'], request_id='related-alpha', backend='native', attach=False)['session_id']
    snapshot = selected.request('session.resume', 'session.snapshot', session_id=sid)
    assert [w['window_id'] for w in snapshot['windows']]==[ids[0]], 'unrelated pre-existing window was captured'
    selected.until('frame', ids[0])
    other = Peer(directory/'node.sock'); peers.append(other)
    other.request('video.enable', 'video.ready', codec='h264')
    beta_sid = other.request('app.launch', 'session.started', app_id=beta['id'], request_id='related-beta', backend='native', attach=False)['session_id']
    other.request('session.resume', 'session.snapshot', session_id=beta_sid); other.until('frame', ids[1])
    selected.send('input.key.logical', window_id=ids[0], key='F2', pressed=True)
    selected.send('input.key.logical', window_id=ids[0], key='F2', pressed=False)
    preview_state = state.with_suffix('.json.preview')
    wait(lambda: preview_state.exists() and json.loads(preview_state.read_text())['pid'] in json.loads(state.read_text()).get('children',[]), 'fixture received remote F2')
    preview_info = json.loads(preview_state.read_text()); preview = preview_info['windows'][0]['id']
    assert preview_info['pid']!=initial['pid']
    created = selected.until('window.created', preview)
    assert created['parent_id']==ids[0]
    frame = selected.until('frame', preview)
    assert frame['codec']=='h264' and base64.b64decode(frame['data']).startswith(b'\0\0\0\1')
    selected.send('input.text.commit', window_id=preview, text='跨进程预览🙂')
    wait(lambda: json.loads(preview_state.read_text())['windows'][0]['text']=='跨进程预览🙂', 'accessory helper received scoped Unicode')
    for key,pressed in (('Control_L',True),('a',True),('a',False),('Control_L',False)):
        selected.send('input.key.logical',window_id=preview,key=key,pressed=pressed)
    copied=selected.request('clipboard.copy','clipboard.data',window_id=preview,request_id='copy-helper')
    assert copied['text']=='跨进程预览🙂', 'accessory helper Edit menu/focus'
    other.send('input.text.commit', window_id=preview, text='forbidden')
    assert other.until('error')['code']=='window_unavailable'
    listed = next(w for w in inventory.request('window.list')['windows'] if w['window_id']==preview)
    assert listed['session_id']==sid and listed['controlled']
    selected.close(); peers.remove(selected)
    wait(lambda: not next(s for s in inventory.request('session.list')['sessions'] if s['session_id']==sid)['controlled'], 'detach releases only this controller')
    selected = Peer(directory/'node.sock'); peers.append(selected)
    selected.request('video.enable', 'video.ready', codec='h264')
    snapshot = selected.request('session.resume', 'session.snapshot', session_id=sid)
    assert {w['window_id'] for w in snapshot['windows']}=={ids[0], preview}
    selected.until('frame', preview)
    selected.send('window.close.request', window_id=preview)
    selected.until('window.destroyed', preview)
    selected.send('input.text.commit', window_id=ids[0], text='主窗口继续工作')
    wait(lambda: json.loads(state.read_text())['windows'][0]['text']=='主窗口继续工作', 'parent survives preview close')
    assert len(json.loads(state.read_text())['windows'])==2
    print('PASS: actual Mac accessory child process, SCK/H264 for both windows, child Unicode, session isolation, catalog ownership, reconnect and independent child close', flush=True)
    for peer in peers: peer.close()
    peers.clear()
    if args.hold:
        (scratch/'ready.json').write_text(json.dumps(dict(publisher=str(directory/'node.sock'), state=str(state), alpha=alpha['id'], alpha_window=ids[0], beta_window=ids[1], session_id=sid, fixture_pid=fixture.pid, node_pid=service.pid)))
        print('READY for external QUIC receiver; create stop file to clean up: '+str(scratch/'stop'), flush=True)
        end = time.monotonic()+1800
        while time.monotonic()<end and not (scratch/'stop').exists(): time.sleep(.2)
finally:
    for peer in peers: peer.close()
    for child in reversed(children):
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGTERM)
            try: child.wait(timeout=5)
            except subprocess.TimeoutExpired: os.killpg(child.pid, signal.SIGKILL); child.wait(timeout=5)
    stop_helpers()
    if temporary: temporary.cleanup()
