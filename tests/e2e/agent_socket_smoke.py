#!/usr/bin/env python3
"""Cloud-only protocol/permission smoke test; does NOT claim GUI interoperability."""
import json, os, pathlib, socket, subprocess, tempfile, time
root=pathlib.Path(__file__).resolve().parents[2]
exe=root/'target/debug/paneferry-agent'
with tempfile.TemporaryDirectory(prefix='paneferry-test-') as tmp:
    home=pathlib.Path(tmp); config=home/'.config/lan-remoteapp'; config.mkdir(parents=True)
    (config/'apps.json').write_text(json.dumps([dict(id='approved',name='Approved',executable='/usr/bin/true',argv=[])]))
    proc=subprocess.Popen([str(exe),'serve'],env={**os.environ,'HOME':tmp,'XDG_DATA_HOME':str(home/'data'),'XDG_DATA_DIRS':str(home/'data')},stderr=subprocess.PIPE)
    sockpath=home/'.local/state/lan-remoteapp/agent.sock'
    try:
        for _ in range(100):
            if sockpath.exists(): break
            time.sleep(.05)
        if not sockpath.exists():
            raise RuntimeError("Agent did not start: "+proc.stderr.read().decode())
        assert sockpath.stat().st_mode & 0o777 == 0o600
        def connect():
            s=socket.socket(socket.AF_UNIX); s.settimeout(3); s.connect(str(sockpath)); return s,s.makefile('rwb',buffering=0)
        s,f=connect()
        def request(v): f.write(json.dumps(v).encode()+b'\n'); return json.loads(f.readline())
        hello=request(dict(type='hello',version=1)); assert hello['type']=='hello' and hello['version']==1
        apps=request(dict(type='app.list',version=1)); assert apps['apps'][0]['id']=='approved'; assert 'executable' not in apps['apps'][0]
        err=request(dict(type='app.launch',version=1,app_id='not-approved',request_id='r1',backend='native')); assert err['type']=='error'
        err=request(dict(type='app.launch',version=1,app_id='approved;id',request_id='r2',backend='native')); assert err['type']=='error'
        assert hello['seq']<apps['seq']<err['seq']
        f.write(b'{"type":"session.detach","version":1}\n'); assert f.readline()==b''; f.close(); s.close()
        s,f=connect(); f.write(b'{"type":"hello","version":99}\n'); assert f.readline()==b'';f.close();s.close()
        s,f=connect(); f.write(b'{"type":"hello","version":1,"command":"id"}\n'); assert f.readline()==b'';f.close();s.close()
        print('PASS: handshake, private socket mode, catalog, unknown app, injection, sequence, detach, wrong version, unknown fields')
    finally:
        proc.terminate(); proc.wait(timeout=5)
    assert not sockpath.exists(), 'socket removed on SIGTERM'
