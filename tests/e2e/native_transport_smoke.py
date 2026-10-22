#!/usr/bin/env python3
"""Actual Swift receiver and clipboard adapter through two real local QUIC nodes.

Only isolated publishers, test files and unique pasteboards are used. No SSH,
installed service, desktop contents or saved device identity is changed.
"""
from pathlib import Path
import json
import os
import socket
import subprocess
import tempfile
import threading
import time

root = Path(__file__).resolve().parents[2]
source = root/'apps/mac-windowhost/Sources/RemoteAppWindowHost'
binary = root/'target/debug/remoteapp-network'
subprocess.run(['cargo','build','-p','remoteapp-network'],cwd=root,check=True)
with tempfile.TemporaryDirectory(prefix='rwn-native-',dir='/tmp') as directory:
    scratch=Path(directory); processes=[]; children=[]; sockets=[]
    def manage(state, **body):
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(20);client.connect(str(state/'control.sock'))
            client.sendall(json.dumps(body).encode()+b'\n')
            result=json.loads(client.makefile('rb').readline(131073))
            assert 'error' not in result,result
            return result
    def ready(state):
        deadline=time.monotonic()+10
        while time.monotonic()<deadline:
            try:return manage(state,op='status')
            except (OSError,ValueError):time.sleep(.05)
        raise AssertionError('network daemon did not start')
    try:
        (scratch/'video.json').write_bytes((root/'tests/fixtures/native-video.json').read_bytes())
        fixture=scratch/'publisher'
        fixture.write_bytes((root/'tests/fixtures/native_publisher.py').read_bytes());fixture.chmod(0o700)
        listener=socket.socket(socket.AF_UNIX);listener.bind(str(scratch/'publisher.sock'));listener.listen();sockets.append(listener)
        def serve():
            while True:
                try: client,_=listener.accept()
                except OSError:return
                sockets.append(client)
                process=subprocess.Popen([str(fixture)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL);children.append(process)
                def pump_in(c=client,p=process):
                    try:
                        while chunk:=c.recv(65536):p.stdin.write(chunk);p.stdin.flush()
                    except (OSError,ValueError):pass
                    finally:
                        try:p.stdin.close()
                        except OSError:pass
                def pump_out(c=client,p=process):
                    try:
                        while chunk:=os.read(p.stdout.fileno(),65536):c.sendall(chunk)
                    except OSError:pass
                    finally:
                        try:c.shutdown(2)
                        except OSError:pass
                        c.close();p.stdout.close();p.wait()
                threading.Thread(target=pump_in,daemon=True).start();threading.Thread(target=pump_out,daemon=True).start()
        threading.Thread(target=serve,daemon=True).start()
        bulk=scratch/'bulk-agent'
        bulk.write_text('''#!/usr/bin/env python3
import pathlib,sys,uuid
root=pathlib.Path(__file__).parent
if sys.argv[1]=='clipboard-upload':
 data=sys.stdin.buffer.read(32*1024*1024)
 identity=uuid.uuid4().hex;(root/identity).write_bytes(data);print(identity,flush=True)
else:sys.stdout.buffer.write((root/sys.argv[2]).read_bytes())
''');bulk.chmod(0o700)
        states=[scratch/'a',scratch/'b']
        for state in states:
            processes.append(subprocess.Popen([str(binary),'serve','--state',str(state),'--publisher',str(scratch/'publisher.sock'),
                '--listen','127.0.0.1:0','--name','fixture','--bulk-agent',str(bulk)],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL))
        a,b=map(ready,states)
        invitation=manage(states[1],op='invite')['invitation']
        manage(states[0],op='pair',invitation=invitation,address=b['listen'])
        deadline=time.monotonic()+5
        while not (states[0]/('r-'+b['device_id']+'.sock')).exists():
            assert time.monotonic()<deadline;time.sleep(.02)
        types=scratch/'WindowHostTypes.swift'
        types.write_text((source/'main.swift').read_text().split('\n// WINDOWHOST_ENTRYPOINT\n')[0])
        test=scratch/'NativeSmoke.swift'
        text=(root/'tests/swift/AutomaticReconnectSmoke.swift').read_text()
        text=text.replace('let options = Options(app: nil, session: "retained", native: NativeRoute(peer: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", state: root.path, helper: root.appendingPathComponent("fake-bridge").path))',
            'let options = try Options.parse(["--native-peer", CommandLine.arguments[2], "--native-state", CommandLine.arguments[3], "--native-helper", CommandLine.arguments[4], "--session", "retained"])')
        text=text.replace('until { app.windows[42] != nil }', 'until { app.windows[42]?.surface.image != nil && app.lastSequence >= 11 }')
        text=text.replace('until { app.lastSequence >= 3 }', 'until { app.lastSequence >= 11 && app.windows[42]?.surface.image != nil }\n        let image = app.windows[42]!.surface.image!\n        let red = NSBitmapImageRep(cgImage:image).colorAt(x:20,y:20)!.usingColorSpace(.sRGB)!\n        precondition(red.redComponent > 0.8 && red.greenComponent < 0.2 && image.width == 640, "QUIC H.264 output is incorrect")\n        let nativeNextSequence = app.lastSequence + 1\n        print("PASS: real H.264 IDR and dependent P frames through QUIC into AppKit; decoded red pixels after reconnect")')
        for offset, seq in enumerate([4,5,6,7]):
            text=text.replace('"seq":'+str(seq)+',', '"seq":nativeNextSequence+'+str(offset)+',')
        test.write_text(text)
        files=[str(file) for file in source.glob('*.swift') if file.name!='main.swift']
        receiver=scratch/'receiver'
        subprocess.run(['swiftc','-module-cache-path','/tmp/lra-network-verification/swift-cache',*files,str(types),str(test),'-o',str(receiver)],check=True,timeout=120)
        checked=subprocess.run([str(receiver),str(scratch),b['device_id'],str(states[0]),str(binary)],check=False,timeout=30,capture_output=True,text=True)
        if checked.returncode:
            print(checked.stdout, checked.stderr[-6000:], flush=True)
            print("fixture requests:", (scratch/"requests").read_text()[-6000:] if (scratch/"requests").exists() else "none", flush=True)
        assert checked.returncode == 0, checked.returncode
        assert 'PASS: real transport' in checked.stdout, (checked.stdout,checked.stderr)
        print(checked.stdout,end='')
        test=scratch/'BulkSmoke.swift'
        text=(root/'tests/swift/ClipboardBulkQUICSmoke.swift').read_text()
        test.write_text(text)
        bulk_test=scratch/'bulk-test'
        subset=['SurfaceView.swift','ClipboardBundle.swift','ClipboardBulkTransfer.swift','RenderGeometry.swift']
        subprocess.run(['swiftc','-module-cache-path','/tmp/lra-network-verification/swift-cache',*[str(source/f) for f in subset],str(test),'-o',str(bulk_test)],check=True,timeout=120)
        subprocess.run([str(bulk_test),b['device_id'],str(states[0]),str(binary)],check=True,timeout=45)
        print('PASS: two real QUIC nodes, Swift receiver reconnect and independent binary bulk without SSH')
    finally:
        for p in processes+children:
            if p.poll() is None:p.terminate()
        for p in processes+children:
            try:p.wait(timeout=5)
            except subprocess.TimeoutExpired:p.kill();p.wait()
        for s in sockets:
            try:s.close()
            except OSError:pass
