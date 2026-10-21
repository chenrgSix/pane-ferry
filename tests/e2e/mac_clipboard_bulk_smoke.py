#!/usr/bin/env python3
"""Opt-in real SSH bulk test; sends generated test bytes only and removes its own remote snapshots."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--host',required=True);parser.add_argument('--user',required=True);parser.add_argument('--port',default='22');parser.add_argument('--agent',default='.local/bin/remoteapp-agent')
args=parser.parse_args();root=Path(__file__).resolve().parents[2];source=root/'apps/mac-windowhost/Sources/RemoteAppWindowHost'
with tempfile.TemporaryDirectory(prefix='remoteapp-bulk-') as directory:
    binary=Path(directory)/'bulk'
    subprocess.run(['swiftc','-module-cache-path',str(Path(directory)/'modules'),str(source/'SurfaceView.swift'),str(source/'ClipboardBundle.swift'),str(source/'ClipboardBulkTransfer.swift'),str(source/'SSHIdentity.swift'),str(source/'RenderGeometry.swift'),str(root/'tests/swift/ClipboardBulkSSHSmoke.swift'),'-o',str(binary)],check=True,timeout=120)
    result=subprocess.run([str(binary),args.host,args.user,args.port,args.agent],check=True,capture_output=True,text=True,timeout=120)
    lines=result.stdout.splitlines();print(lines[0])
    ids=json.loads(lines[-1])['ids']
    cleanup='''import json,re,shutil
from pathlib import Path
ids=json.loads(%r)
root=Path.home()/'.cache/lan-remoteapp/clipboard'
for identity in ids:
 assert re.fullmatch('[0-9a-f]{32}',identity)
 path=root/identity
 assert path.is_dir() and not path.is_symlink()
 shutil.rmtree(path)
'''%json.dumps(ids)
    subprocess.run(['ssh','-F','/dev/null','-T','-S','~/.ssh/lan-remoteapp-%C','-oBatchMode=yes','-oStrictHostKeyChecking=yes','-p',args.port,'-l',args.user,'--',args.host,'python3 -'],input=cleanup,text=True,check=True,timeout=15)
