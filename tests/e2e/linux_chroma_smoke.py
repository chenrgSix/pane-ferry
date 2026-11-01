#!/usr/bin/env python3
"""Decode Windows golden packets and Linux probes through the production receiver."""
import json
from pathlib import Path
import sys
import threading

root = Path(__file__).resolve().parents[2]
sys.path.insert(0,str(root/'apps/linux-client'))
from video_color import carrier_size, negotiate, unpack_nv12
from windowhost import Video

assert negotiate('444-atlas-v2',[]) == '420'
assert negotiate('444-atlas-v2',['video.color.444-atlas-v1']) == '444-atlas-v1'
for dims in ((0,2),(True,2),(4097,2),(4096,4096)):
    try: carrier_size(*dims); raise AssertionError('invalid dimensions accepted')
    except ValueError: pass
for mode in ('444-atlas-v1','444-atlas-v2'):
    ew,eh=carrier_size(3,3,mode); stride=(ew+3)&~3
    carrier=bytes([11])*8+bytes([22])*8+bytes([33])*8+bytes([44])*8+bytes([55])*(ew*eh*3//2-32)
    assert len(unpack_nv12(carrier,stride,3,3,mode))==48

paths = sorted((root/'tests/fixtures').glob('windows-444*.json'))
for argument in sys.argv[1:]: paths.extend(sorted(Path(argument).glob('*.json')))
for path in paths:
    event=json.loads(path.read_text())
    decoded=[]; ready=threading.Event()
    def receive(pixels, frame_id):
        decoded.append(pixels); ready.set()
    video=Video(receive,event['color'])
    try:
        event.update(stream_id=1,key_frame=True,frame_id=1)
        video.push(event)
        assert ready.wait(10),f'{path.name}: decoder did not produce an image'
        pixels=decoded[-1]; raw=pixels.get_pixels(); stride=pixels.get_rowstride()
        assert (pixels.get_width(),pixels.get_height())==(event['width'],event['height'])
        # These fixtures contain alternating red/blue one-pixel-wide columns.
        for y in (4,event['height']//2,event['height']-5):
            for x in range(8,min(event['width']-8,100)):
                r,g,b=raw[y*stride+x*3:y*stride+x*3+3]
                assert (r-b if x%2==0 else b-r)>180,(path.name,x,y,r,g,b)
        print('PASS',path.name,event['color'],event['width'],event['height'])
    finally: video.close()
print('PASS: carrier bounds, negotiation and full-chroma decoding')
