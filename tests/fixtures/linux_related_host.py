#!/usr/bin/env python3
"""Disposable Xvfb desktop, related-window fixture, Agent and QUIC endpoint."""
import argparse
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import subprocess
import time

root=Path(__file__).resolve().parents[2]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--directory',type=Path,required=True)
args=parser.parse_args();directory=args.directory.resolve();directory.mkdir(parents=True,exist_ok=True,mode=0o700)
children=[];stopping=False
def stop(*_):
    global stopping
    stopping=True
signal.signal(signal.SIGTERM,stop);signal.signal(signal.SIGINT,stop)
def start(command,env,name):
    with (directory/(name+'.log')).open('w') as log:
        process=subprocess.Popen(command,env=env,stdout=log,stderr=log,start_new_session=True)
    children.append(process);return process
def wait(predicate):
    end=time.monotonic()+15
    while not predicate():
        assert time.monotonic()<end and not stopping
        time.sleep(.05)
def request(path,body):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(10);client.connect(str(path));client.sendall(json.dumps(body).encode()+b'\n')
        return json.loads(client.makefile('rb').readline())
def ready(path,body):
    try:return request(path,body)
    except (OSError,ValueError):return None
try:
    for name in ('stop','ready.json','fixture.json','fixture.child.json'):
        (directory/name).unlink(missing_ok=True)
    home=directory/'home';home.mkdir(exist_ok=True)
    (directory/'data/applications').mkdir(parents=True,exist_ok=True)
    for name in ('themes','openbox','icons','mime','glycin-loaders'):
        source=Path('/usr/share')/name;target=directory/'data'/name
        if source.is_dir() and not target.exists():target.symlink_to(source,target_is_directory=True)
    authority=directory/'Xauthority';authority.touch(mode=0o600)
    display=next(n for n in range(3100,3300) if not Path(f'/tmp/.X11-unix/X{n}').exists())
    subprocess.run(['xauth','-f',str(authority),'add',f':{display}','MIT-MAGIC-COOKIE-1',secrets.token_hex(16)],check=True)
    env=dict(os.environ,HOME=str(home),DISPLAY=f':{display}',XAUTHORITY=str(authority),GDK_BACKEND='x11',GTK_IM_MODULE='simple',
             XDG_DATA_HOME=str(directory/'data'),XDG_DATA_DIRS=str(directory/'data'))
    for key in ('WAYLAND_DISPLAY','DBUS_SESSION_BUS_ADDRESS','SESSION_MANAGER'):env.pop(key,None)
    start(['Xvfb',f':{display}','-screen','0','1600x900x24','-nolisten','tcp','-auth',str(authority)],env,'xvfb')
    wait(lambda:Path(f'/tmp/.X11-unix/X{display}').exists())
    start(['openbox'],env,'openbox');time.sleep(.4)
    state=directory/'fixture.json'
    fixture=start(['/usr/bin/python3',str(root/'tests/e2e/discovery_fixture.py'),str(state)],env,'fixture')
    start([str(root/'target/debug/paneferry-agent'),'serve'],env,'agent')
    publisher=home/'.local/state/lan-remoteapp/agent.sock'
    wait(lambda:ready(publisher,dict(type='hello',version=1)));wait(state.exists)
    network=directory/'network'
    start([str(root/'target/debug/paneferry-network'),'serve','--state',str(network),'--publisher',str(publisher),'--listen','0.0.0.0:0'],env,'network')
    wait(lambda:ready(network/'control.sock',dict(op='status')))
    status=request(network/'control.sock',dict(op='status'))
    (directory/'ready.json').write_text(json.dumps(dict(status=status,state=str(state),display=display,fixture_pid=fixture.pid)))
    print('READY: isolated Linux related-window host',flush=True)
    end=time.monotonic()+1800
    while not stopping and time.monotonic()<end and not (directory/'stop').exists():time.sleep(.2)
finally:
    for child in reversed(children):
        if child.poll() is None:
            os.killpg(child.pid,signal.SIGTERM)
            try:child.wait(timeout=5)
            except subprocess.TimeoutExpired:os.killpg(child.pid,signal.SIGKILL);child.wait(timeout=5)
