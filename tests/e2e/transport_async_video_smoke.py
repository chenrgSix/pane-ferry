#!/usr/bin/env python3
"""A real pipe/reader with stalled injected decoder and ordered lifecycle messages."""
from pathlib import Path
import subprocess
import tempfile
import sys
root = Path(__file__).resolve().parents[2]
source = root / 'apps/mac-windowhost/Sources/PaneFerryWindowHost'
with tempfile.TemporaryDirectory(prefix='paneferry-transport-video-') as directory:
    scratch = Path(directory)
    bridge = scratch / 'bridge'
    bridge.write_text(f'#!{sys.executable}\n' + '''import json,time,pathlib
p=pathlib.Path(__file__).parent
w={'window_id':1,'width':2,'height':2,'geometry_revision':1,'visible':True}
def send(o): o['version']=1; print(json.dumps(o),flush=True)
send({'type':'session.snapshot','windows':[w]})
send(dict({k:v for k,v in w.items() if k!='visible'},type='frame',codec='h264',stream_id=1,encoded_width=2,encoded_height=2,data='AA==',frame_id=1))
end=time.monotonic()+3
while not (p/'decoding').exists() and time.monotonic()<end: time.sleep(.01)
send({'type':'clipboard.data','text':'fixture'})
send({'type':'window.destroyed','window_id':1})
send(dict(w,type='window.created'))
send(dict({k:v for k,v in w.items() if k!='visible'},type='frame',codec='h264',stream_id=2,encoded_width=2,encoded_height=2,data='AQ==',frame_id=2))
time.sleep(8)
''')
    bridge.chmod(0o700)
    types = scratch / 'Types.swift'
    types.write_text((source / 'main.swift').read_text().split('\n// WINDOWHOST_ENTRYPOINT\n')[0])
    binary = scratch / 'smoke'
    files = [str(p) for p in source.glob('*.swift') if p.name != 'main.swift']
    subprocess.run(['swiftc', '-module-cache-path', str(scratch / 'modules'), *files, str(types),
                    str(root / 'tests/swift/TransportAsyncVideoSmoke.swift'), '-o', str(binary)], check=True, timeout=120)
    subprocess.run([str(binary), str(scratch)], check=True, timeout=15)
