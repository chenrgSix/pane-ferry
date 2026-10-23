#!/usr/bin/env python3
"""Mac-only quality negotiation/switching with an isolated RWP publisher."""
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[2]
source = root/'apps/mac-windowhost/Sources/PaneFerryWindowHost'
with tempfile.TemporaryDirectory(prefix='paneferry-display-quality-') as directory:
    scratch = Path(directory)
    types = scratch/'WindowHostTypes.swift'
    types.write_text((source/'main.swift').read_text().split('\n// WINDOWHOST_ENTRYPOINT\n')[0])
    (scratch/'video.json').write_bytes((root/'tests/fixtures/native-video.json').read_bytes())
    fixture = scratch/'publisher'
    fixture.write_text('''#!/usr/bin/env python3
import base64,json,pathlib,struct,sys,zlib
root=pathlib.Path(__file__).parent
counter=root/'connections'; n=int(counter.read_text())+1 if counter.exists() else 1; counter.write_text(str(n))
video=False
def png(w,h):
 def chunk(kind,data): return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data))
 raw=(b'\\x00'+bytes([255,0,0])*w)*h
 return base64.b64encode(b'\\x89PNG\\r\\n\\x1a\\n'+chunk(b'IHDR',struct.pack('>2I5B',w,h,8,2,0,0,0))+chunk(b'sRGB',bytes([0]))+chunk(b'IDAT',zlib.compress(raw))+chunk(b'IEND',b'')).decode()
def emit(**value): print(json.dumps(dict(value,version=1)),flush=True)
with (root/'requests').open('a',buffering=1) as log:
 for line in sys.stdin:
  item=json.loads(line); log.write(json.dumps(dict(item,connection=n))+'\\n')
  if item['type']=='hello': emit(type='hello',capabilities=['input.text.commit']+([] if (root/'no-video').exists() else ['native.h264','native.h264-refinement']))
  if item['type']=='video.enable': video=True; emit(type='video.ready',codec='h264')
  if item['type']=='session.resume':
   windows=[dict(window_id=42,title='Quality fixture',width=640,height=480,geometry_revision=n,scale_factor=1,visible=True,role='toplevel'),
            dict(window_id=43,parent_id=42,title='Related fixture',width=32,height=32,geometry_revision=n,scale_factor=1,visible=True,role='popup')]
   emit(type='session.snapshot',session_id='retained',seq=1,windows=windows)
   frames=json.loads((root/'video.json').read_text())['frames'][:1] if video else [png(640,480)]
   for index,frame in enumerate(frames):
    fields=dict(codec='h264',data=frame,stream_id=n,key_frame=True,encoded_width=640,encoded_height=480) if video else dict(png=frame)
    emit(type='frame',session_id='retained',seq=2+index,window_id=42,geometry_revision=n,width=640,height=480,**fields)
   emit(type='frame',session_id='retained',seq=3,window_id=43,geometry_revision=n,width=32,height=32,png=png(32,32))
  if item['type']=='session.detach': break
''')
    fixture.chmod(0o700)
    files = [str(path) for path in source.glob('*.swift') if path.name != 'main.swift']
    binary = scratch/'smoke'
    subprocess.run(['swiftc', '-module-cache-path', '/tmp/lra-network-verification/swift-cache', *files, str(types),
                    str(root/'tests/swift/DisplayQualitySmoke.swift'), '-o', str(binary)], check=True, timeout=120)
    subprocess.run([str(binary), str(scratch)], check=True, timeout=35)
