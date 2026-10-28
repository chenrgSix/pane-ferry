"""Focused native PipeWire pause/refresh probe in wayland_smoke's private GNOME."""
import json


def verify(peer, target, state, app):
    peer.send(type='video.configure', max_fps=30)
    started = peer.request('app.launch', 'session.started', app_id=target['id'], request_id='activity-native', backend='native', scale_factor=1, attach=False)
    snapshot = peer.request('session.resume', 'session.snapshot', session_id=started['session_id'])
    metadata = snapshot['windows'][0]
    first = peer.until('frame')
    assert metadata['scale_factor'] == 1
    assert first['key_frame'] and first['window_id'] == metadata['window_id']
    assert json.loads(state.read_text())['scale'] == 1
    assert peer.request('video.activity', 'video.activity.changed', state='suspended')['state'] == 'suspended'
    paused_frames = peer.frame_count
    sessions = peer.request('session.list')['sessions']
    assert any(s['session_id'] == started['session_id'] and s['controlled'] for s in sessions)
    peer.send(type='window.configure', window_id=metadata['window_id'], width=800, height=500)
    peer.until('window.metadata')
    assert peer.request('video.activity', 'video.activity.changed', state='foreground')['state'] == 'foreground'
    assert peer.frame_count == paused_frames, 'no video may pass between suspension and resume acknowledgement'
    resumed = peer.until('frame')
    assert resumed['key_frame'] and resumed['stream_id'] != first['stream_id']
    # Mutter/GTK can report a one-pixel intermediate crop while completing a
    # resize. Reject the cached 700x487 frame, without treating that rounding as
    # stale video (the Mac receiver uses the same bounded size reconciliation).
    assert abs(resumed['width']-800) <= 1 and abs(resumed['height']-500) <= 1, (resumed['width'], resumed['height'])
    assert (resumed['width'], resumed['height']) != (first['width'], first['height'])
    assert app.poll() is None and json.loads(state.read_text())['text'] == 'preserved fixture contents'
    peer.send(type='session.detach')
    print('PASS: native Wayland 1x/30 cap, paused metadata/resize without video, new keyframe at current geometry, preserved source process/content', flush=True)
