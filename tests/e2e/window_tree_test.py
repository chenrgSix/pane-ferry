"""Real GNOME/PipeWire protocol coverage for a toolkit-generated window family."""
import json
import time


def verify(peer, sock, env, state, app, wait, Peer, Bridge):
    bridge = Bridge(env)
    command = state.with_suffix('.command')
    serial = 0
    def action(name):
        nonlocal serial
        serial += 1
        temp = command.with_suffix('.new')
        temp.write_text(json.dumps({'serial': serial, 'action': name})); temp.replace(command)
    def inventory():
        return [w for w in bridge.call(op='list')['result'] if w['pid'] == app.pid]
    root = next(w for w in inventory() if w['parent_id'] is None)
    assert root['bridge_version'] >= 3 and root['window_kind'] == 'window', root
    target = next(w for w in peer.request('window.list')['windows'] if w['window_id'] == root['window_id'])
    started = peer.request('app.launch', 'session.started', app_id=target['id'], request_id='window-tree', backend='native')
    sid = started['session_id']; root_id = root['window_id']
    metadata, frames = {}, set()
    def collect(until):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            raw = peer.file.readline(16*1024*1024+1)
            assert raw.endswith(b'\n'), 'controller disconnected'
            event = json.loads(raw)
            assert event['type'] not in ('error', 'session.exited'), event
            kind = event['type']; wid = event.get('window_id')
            if kind in ('window.created', 'window.metadata'): metadata[wid] = event
            elif kind == 'window.destroyed': metadata.pop(wid, None); frames.discard(wid)
            elif kind == 'frame':
                assert event['codec'] == 'h264'
                assert event['width'] == metadata[wid]['width'] and event['height'] == metadata[wid]['height']
                frames.add(wid)
            if until(): return
        raise AssertionError(('window tree timed out', list(metadata), frames))
    try:
        collect(lambda: root_id in frames)
        for cycle in range(2):
            action('open')
            collect(lambda: len(metadata) == 4 and len(frames) == 4)
            current = {w['window_id']: w for w in inventory()}
            children = [w for wid, w in metadata.items() if wid != root_id]
            assert sorted(w['role'] for w in children) == ['dialog', 'popup', 'popup'], children
            assert any(w['parent_id'] != root_id for w in children), 'nested relationship was flattened'
            for wid, event in metadata.items():
                info = current[wid]; placement = event['presentation']
                assert [placement[k] for k in ('x','y','width','height')] == [info[k] for k in ('x','y','width','height')]
                assert placement['kind'] == info['window_kind']
                assert event['width'] == round(info['width']*info['scale']) and event['height'] == round(info['height']*info['scale'])
            action('close')
            collect(lambda: len(metadata) == 1 and root_id in metadata)
        # Vanishing before the stream is ready is ordinary popup lifecycle.
        for _ in range(12):
            action('pulse'); time.sleep(.06)
        action('close')
        # A metadata round-trip on a root resize proves the same controller is
        # still live after all capture-creation/destruction races.
        peer.send(type='window.configure', window_id=root_id, width=1200, height=700)
        frames.discard(root_id)
        collect(lambda: root_id in frames and metadata[root_id]['width'] == 1200)
        assert app.poll() is None
        peer.close()
        resumed = Peer(sock)
        try:
            snapshot = resumed.request('session.resume', 'session.snapshot', session_id=sid)
            assert len(snapshot['windows']) == 1 and snapshot['windows'][0]['window_id'] == root_id
            assert snapshot['windows'][0]['presentation']['kind'] == 'window'
            resumed.until('frame')
        finally: resumed.close()
        print('PASS: actual GNOME type/position/parent metadata, nested popup and dialog H.264 streams, short-lived popup races, surviving root input loop and reconnect')
    except Exception:
        print('Window tree failure:', json.dumps({'metadata': metadata, 'frames': list(frames), 'inventory': inventory()}, ensure_ascii=False), flush=True)
        raise
    finally:
        action('close'); bridge.close()
