#!/usr/bin/env python3
"""Real Linux service: device persistence, isolated return routes, stop-sharing."""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import uuid

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--agent',required=True,type=Path)
args=parser.parse_args()
with tempfile.TemporaryDirectory(prefix='lra-n-') as temporary:
    home=Path(temporary);env=dict(os.environ,HOME=str(home))
    path=home/'.local/state/lan-remoteapp/agent.sock'
    def start():
        child=subprocess.Popen([str(args.agent.resolve()),'serve'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        for _ in range(100):
            if path.exists():
                try:
                    test=socket.socket(socket.AF_UNIX);test.connect(str(path));test.close();return child
                except OSError:pass
            time.sleep(.05)
        raise AssertionError('service unavailable')
    def request(kind,**fields):
        peer=socket.socket(socket.AF_UNIX);peer.settimeout(5);peer.connect(str(path));reader=peer.makefile('rb')
        try:
            peer.sendall(b'{"type":"hello","version":1}\n');json.loads(reader.readline())
            peer.sendall((json.dumps(dict(type=kind,version=1,**fields))+'\n').encode())
            return json.loads(reader.readline())
        finally:reader.close();peer.close()
    holds=[];listeners=[];agent=start()
    try:
        identity=request('node.info')['device_id'];uuid.UUID(identity)
        a,b=str(uuid.uuid4()),str(uuid.uuid4())
        def hold(device):
            offer=request('peer.prepare',device_id=device)
            assert offer['type']=='peer.prepared' and len(offer['socket'].encode())<104
            listener=socket.socket(socket.AF_UNIX);listener.bind(offer['socket']);listener.listen();listeners.append(listener)
            child=subprocess.Popen([str(args.agent.resolve()),'peer-hold'],env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
            child.stdin.write((json.dumps(dict(device_id=device,name='fixture',generation=offer['generation']))+'\n').encode());child.stdin.flush()
            assert child.stdout.readline()==b'ready\n';holds.append(child)
            record=path.parent/'peers'/f'{device}.json'
            assert record.stat().st_mode&0o777==0o600
            return child,record,json.loads(record.read_text())
        a1,record_a,first=hold(a);b1,record_b,other=hold(b);a2,_,second=hold(a)
        assert first['socket']!=second['socket']!=other['socket']
        a1.stdin.close();a1.wait(timeout=3)
        assert json.loads(record_a.read_text())['generation']==second['generation']
        assert record_b.exists()
        a2.stdin.close();a2.wait(timeout=3)
        assert not record_a.exists() and record_b.exists()
        assert request('peer.prepare',device_id='../escape')['type']=='error'
        request('sharing.set',enabled=False)
        assert request('app.list')['code']=='sharing_disabled'
        assert request('node.info')['sharing_enabled'] is False
        agent.terminate();agent.wait(timeout=5);agent=start()
        info=request('node.info');assert info['device_id']==identity and info['sharing_enabled'] is False
        print('PASS: persistent device identity, independent peer routes, replacement cleanup, sharing isolation')
    finally:
        for child in holds:
            if child.poll() is None:
                child.stdin.close();child.wait(timeout=3)
        for listener in listeners:listener.close()
        agent.terminate();agent.wait(timeout=5)
