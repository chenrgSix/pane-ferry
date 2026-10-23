"""Text delivery checks for the test-owned GNOME compositor only.

The caller owns the bus, application and state file. Direct Mutter keyboard
calls are a fixture driver, not a product alternative to Portal authorization.
"""
import json
import importlib.util
import os
from pathlib import Path
import time
from gi.repository import Gio, GLib


def verify(env, pid, state, Bridge, wait, expect_method, delayed_paste=False):
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    service = 'org.gnome.Mutter.RemoteDesktop'
    session = bus.call_sync(service, '/org/gnome/Mutter/RemoteDesktop', service, 'CreateSession',
                            None, None, Gio.DBusCallFlags.NO_AUTO_START, 2000, None).unpack()[0]

    def call(method, params=None):
        return bus.call_sync(service, session, service+'.Session', method, params, None,
                             Gio.DBusCallFlags.NO_AUTO_START, 2000, None)

    source = Path(__file__).resolve().parents[2]/'apps/linux-agent/src/desktop_input.py'
    spec = importlib.util.spec_from_file_location('fixture_eis',source)
    module = importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    eis = None

    def key(code, down):
        eis.send({'type':'key','owner':1,'code':code+8,'pressed':down})

    def stroke(code, control=False, legacy=False):
        if control:
            key(29, True)
            if legacy: time.sleep(.012)
        key(code, True)
        if legacy: time.sleep(.012)
        key(code, False)
        if control:
            if legacy: time.sleep(.012)
            key(29, False)

    bridge = Bridge(env)
    original = json.loads(state.read_text())['text']
    try:
        call('Start')
        answer,fds = bus.call_with_unix_fd_list_sync(service,session,service+'.Session','ConnectToEIS',
            GLib.Variant('(a{sv})',({'device-types':GLib.Variant('u',3)},)), None,
            Gio.DBusCallFlags.NO_AUTO_START,2000,None,None)
        fd = fds.get(answer.unpack()[0])
        try: eis = module.Eis(fd)
        finally: os.close(fd)
        wait(lambda:(eis.dispatch(),eis.ready())[1])
        info = next(w for w in bridge.call(op='list')['result'] if w['pid']==pid)
        wid = info['window_id']
        assert 'result' in bridge.call(op='acquire',id=info['id'],pid=pid)
        assert 'result' in bridge.call(op='focus',window_id=wid)
        time.sleep(.1)
        if delayed_paste:
            stroke(30, control=True)
            eis.sync()
            old = 'about:newtab-0123456789'
            for character in old:
                assert 'result' in bridge.call(op='set_text',window_id=wid,text=character)
                time.sleep(.016)
                assert 'result' in bridge.call(op='focus',window_id=wid)
                stroke(47, control=True,legacy=True)
            time.sleep(.4)
            actual = json.loads(state.read_text())['text']
            assert actual != old, 'the delayed client must expose the old clipboard overwrite race'
            print('REPRODUCED: old per-commit clipboard path changed', repr(old), 'to', repr(actual), flush=True)
        marker = 'private fixture clipboard marker'
        assert 'result' in bridge.call(op='set_text',window_id=wid,text=marker)
        if expect_method is None:
            before = json.loads(state.read_text())
            for text in ['about:newtab', '中文🙂', 'must not submit\n']:
                rejected = bridge.call(op='commit_text',window_id=wid,text=text)
                assert rejected.get('error')=='wayland_text_unavailable', rejected
            time.sleep(.15)
            after = json.loads(state.read_text())
            assert (after['text'],after['activated'])==(before['text'],before['activated'])
            assert bridge.call(op='get_text',window_id=wid)['result']['text']==marker
            # Explicit whole-text paste remains supported in this context.
            stroke(30, control=True)
            eis.sync()
            assert 'result' in bridge.call(op='set_text',window_id=wid,text=original)
            stroke(47, control=True)
            wait(lambda:json.loads(state.read_text())['text']==original)
            print('PASS: missing text-input context rejects before editing, clipboard untouched, explicit paste works',flush=True)
            return
        for run in range(3):
            stroke(30, control=True)
            eis.sync()
            parts = ['about', ':', 'newtab', '-Aa09/?:; ', '中文🙂', '连续输入 ', str(run)] * 4
            expected = ''.join(parts)
            for part in parts:
                # Same ordering barrier used by the native Wayland backend.
                eis.sync()
                reply = bridge.call(op='commit_text',window_id=wid,text=part)
                assert reply.get('result',{}).get('method')==expect_method, reply
            # A real key immediately after the commits must observe all text.
            stroke(28)
            try:
                wait(lambda: json.loads(state.read_text()).get('activated',[])[-1:]==[expected],seconds=3)
            except AssertionError:
                raise AssertionError({'run':run,'expected':expected,'actual':json.loads(state.read_text())}) from None
            assert json.loads(state.read_text())['text']==expected
        assert bridge.call(op='get_text',window_id=wid)['result']['text']==marker
        invalid = bridge.call(op='commit_text',window_id=wid+100000,text='wrong window')
        assert invalid.get('error')=='window_unavailable', invalid
        stroke(30, control=True)
        eis.sync()
        assert 'result' in bridge.call(op='commit_text',window_id=wid,text=original)
        try: wait(lambda:json.loads(state.read_text())['text']==original,seconds=3)
        except AssertionError: raise AssertionError({'restore':original,'actual':json.loads(state.read_text())}) from None
        print('PASS:',expect_method,'three continuous Unicode/punctuation bursts, following Return ordering, untouched clipboard and scoped target',flush=True)
    finally:
        bridge.close()
        if eis: eis.close()
        call('Stop')
