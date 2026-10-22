#!/usr/bin/env python3
"""Exercise the production Python receiver's native/SSH route choice without GTK."""
from pathlib import Path
import json
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid
root=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(root/'apps/linux-client'))
import wire
binary=root/'target/debug/remoteapp-network'
subprocess.run(['cargo','build','-p','remoteapp-network'],cwd=root,check=True)
with tempfile.TemporaryDirectory(prefix='rwn-routes-',dir='/tmp') as temporary:
    state=Path(temporary);processes=[];listeners=[]
    wire.STATE=state/'a'
    def publisher(path,name):
        path.parent.mkdir(parents=True,exist_ok=True)
        listener=socket.socket(socket.AF_UNIX);listener.bind(str(path));listener.listen();listeners.append(listener)
        def serve():
            while True:
                try:client,_=listener.accept()
                except OSError:return
                def handle(client):
                    try:
                        with client,client.makefile('rb') as stream:
                            for line in stream:
                                event=json.loads(line)
                                client.sendall(json.dumps(dict(type=event['type'],version=1,name=name)).encode()+b'\n')
                    except OSError:pass
                threading.Thread(target=handle,args=(client,),daemon=True).start()
        threading.Thread(target=serve,daemon=True).start()
    def manage(directory,**body):
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(10);client.connect(str(directory/'control.sock'))
            client.sendall(json.dumps(body).encode()+b'\n')
            response=json.loads(client.makefile('rb').readline())
            assert 'error' not in response,response
            return response
    try:
        publisher(state/'publisher.sock','native')
        directories=[wire.STATE/'network',state/'b']
        for directory in directories:
            processes.append(subprocess.Popen([str(binary),'serve','--state',str(directory),'--publisher',str(state/'publisher.sock'),'--listen','127.0.0.1:0'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL))
        for directory in directories:
            deadline=time.monotonic()+10
            while not (directory/'control.sock').exists():
                assert time.monotonic()<deadline;time.sleep(.02)
        remote=manage(directories[1],op='status');peer=remote['device_id']
        invite=manage(directories[1],op='invite')['invitation']
        manage(directories[0],op='pair',invitation=invite,address=remote['listen'])
        deadline=time.monotonic()+5
        while not (directories[0]/('r-'+peer+'.sock')).exists():
            assert time.monotonic()<deadline;time.sleep(.02)
        generation=uuid.uuid4().hex;ssh_path=wire.STATE/'peers'/('p'+generation+'.sock')
        publisher(ssh_path,'ssh')
        wire.save(wire.STATE/'peers'/(peer+'.json'),dict(version=1,device_id=peer,name='SSH',generation=generation,socket=str(ssh_path)))
        assert len(wire.peers())==2
        assert wire.request('quic:'+peer,'app.list')['name']=='native'
        assert wire.request(peer,'app.list')['name']=='ssh'
        bridge=subprocess.Popen([str(binary),'bridge','--socket',str(directories[0]/('r-'+peer+'.sock'))],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
        processes.append(bridge)
        bridge.stdin.write(b'{"type":"hello","version":1}\n');bridge.stdin.flush()
        assert json.loads(bridge.stdout.readline())['type']=='hello'
        manage(directories[0],op='revoke',device_id=peer)
        bridge.wait(timeout=5)  # parent intentionally still holds stdin open
        bridge.stdin.close();bridge.stdout.close()
        try:wire.request('quic:'+peer,'app.list');raise AssertionError('revoked native route accepted')
        except ValueError:pass
        assert wire.request(peer,'app.list')['name']=='ssh','revocation changed explicit SSH route'
        print('PASS: production Python Wire over native QUIC; native and SSH routes stay explicit; revocation cannot silently fall back')
    finally:
        for p in processes:
            p.terminate();p.wait(timeout=5)
        for s in listeners:s.close()
