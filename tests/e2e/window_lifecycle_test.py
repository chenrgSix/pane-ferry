"""Close test-owned clients while private GNOME owns their virtual output."""
import json
import subprocess
from concurrent.futures import ThreadPoolExecutor


def verify(env, app, state, fixture, scratch, log, start, wait, Bridge, monitor_layout, shell):
    baseline = monitor_layout()
    preserved = json.loads(state.read_text())['text']

    def healthy():
        assert shell.poll() is None, f'private GNOME crashed: {shell.returncode}'
        assert app.poll() is None, 'unrelated fixture exited'
        assert json.loads(state.read_text())['text'] == preserved
        return True

    def cleaned():
        healthy()
        return monitor_layout() == baseline

    def victim(index):
        record = scratch / f'lifecycle-{index}.json'
        process = start(['/usr/bin/python3', str(fixture), str(record), '--other'],
                        dict(env, GDK_BACKEND='wayland'), log)
        wait(record.exists)
        bridge = Bridge(env)
        info = wait(lambda: next((w for w in bridge.call(op='list')['result'] if w['pid'] == process.pid), None))
        return process, bridge, info

    # This reproduces the production crash: release is invoked by unmanaged,
    # after the compositor has discarded the root's native monitor.
    for index in range(3):
        process, bridge, info = victim(index)
        try:
            reply = bridge.call(op='acquire', id=info['id'], pid=process.pid, scale=1)
            assert 'result' in reply, reply
            assert len(monitor_layout()) == len(baseline) + 1
            reply = bridge.call(op='close', window_id=info['window_id'])
            assert 'result' in reply, reply
            process.wait(timeout=8)
            wait(cleaned)
        finally:
            bridge.close()
        print('PASS: managed window closed, private GNOME and unrelated fixture preserved', index, flush=True)

    for index in range(3, 6):
        process, bridge, info = victim(index)
        try:
            with ThreadPoolExecutor(max_workers=1) as pool:
                opening = pool.submit(bridge.call, op='acquire', id=info['id'], pid=process.pid, scale=2)
                # Kill only the test fixture once output provisioning is visible.
                wait(lambda: len(monitor_layout()) > len(baseline))
                process.kill(); process.wait(timeout=8)
                reply = opening.result(timeout=10)
                assert 'result' in reply or reply.get('error') in ('window_unavailable', 'wayland_bridge_failed'), reply
            wait(cleaned)
        finally:
            bridge.close()
        print('PASS: client exits during acquisition, output cleaned without desktop loss', index, flush=True)

    for mode in ('release', 'idle', 'disable'):
        bridge = Bridge(env)
        try:
            info = next(w for w in bridge.call(op='list')['result'] if w['pid'] == app.pid)
            reply = bridge.call(op='acquire', id=info['id'], pid=app.pid, scale=1)
            assert 'result' in reply, reply
            if mode == 'release':
                bridge.close(); bridge = None
            elif mode == 'disable':
                subprocess.run(['gnome-extensions', 'disable', 'paneferry@local'], env=env, check=True)
            wait(cleaned)
            if mode != 'disable':
                observer = Bridge(env)
                try:
                    restored = next(w for w in observer.call(op='list')['result'] if w['pid'] == app.pid)
                    assert tuple(restored[k] for k in ('x', 'y', 'width', 'height')) == tuple(info[k] for k in ('x', 'y', 'width', 'height'))
                finally:
                    observer.close()
        finally:
            if bridge:
                bridge.close()
        print('PASS: living application preserved through', mode, flush=True)
    healthy()
    print('NOT TESTED: user desktop, real Feishu/ChatGPT, clarity or long-running tasks', flush=True)
