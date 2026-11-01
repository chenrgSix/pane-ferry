#!/usr/bin/env python3
"""Private PipeWire/Pulse/null-sink audio only; never opens login audio devices."""
import json
import os
from pathlib import Path
import subprocess
import socket
import sys
import tempfile
import time

root=Path(__file__).resolve().parents[2]
if '--private-bus' not in sys.argv:
    with tempfile.TemporaryDirectory(prefix='paneferry-audio-bus-') as bus:
        conf=Path(bus)/'session.conf'
        conf.write_text('<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen><policy context="default"><allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/></policy></busconfig>')
        raise SystemExit(subprocess.call(['dbus-run-session','--config-file='+str(conf),'--',sys.executable,__file__,'--private-bus']))
with tempfile.TemporaryDirectory(prefix='paneferry-audio-') as directory, tempfile.TemporaryDirectory(prefix='pfah-') as home:
    scratch=Path(directory); config=scratch/'config'; runtime=scratch/'runtime'; runtime.mkdir(mode=0o700)
    extra=config/'wireplumber/wireplumber.conf.d'; extra.mkdir(parents=True)
    (extra/'99-test.conf').write_text('''wireplumber.profiles = { main = {
        monitor.alsa = disabled
        monitor.bluez = disabled
        monitor.v4l2 = disabled
        monitor.libcamera = disabled
    } }\n''')
    env=dict(os.environ,CARGO_HOME=os.environ.get('CARGO_HOME',str(Path.home()/'.cargo')),HOME=home,XDG_CONFIG_HOME=str(config),XDG_STATE_HOME=str(scratch/'state'),
        XDG_RUNTIME_DIR=str(runtime),PIPEWIRE_RUNTIME_DIR=str(runtime),PIPEWIRE_REMOTE='pipewire-0',
        PULSE_SERVER='unix:'+str(runtime/'pulse/native'))
    processes=[]; logs=[]
    def start(command,name):
        log=open(scratch/(name+'.log'),'wb'); logs.append(log)
        process=subprocess.Popen(command,env=env,stdout=log,stderr=log); processes.append(process); return process
    def wait(predicate):
        until=time.monotonic()+10
        while not predicate():
            assert all(p.poll() is None for p in processes),'private audio service exited'
            assert time.monotonic()<until,'private audio service timeout'
            time.sleep(.05)
    def pulse(*args):
        return subprocess.check_output(['pactl',*args],env=env,text=True,stderr=subprocess.DEVNULL,timeout=5).strip()
    try:
        start(['pipewire'],'pipewire'); wait(lambda:(runtime/'pipewire-0').exists())
        start(['wireplumber'],'wireplumber')
        start(['pipewire-pulse'],'pulse'); wait(lambda:(runtime/'pulse/native').exists())
        pulse('load-module','module-null-sink','sink_name=pf_output','channels=2','rate=48000')
        pulse('load-module','module-null-sink','sink_name=pf_unrelated','channels=2','rate=48000')
        pulse('set-default-sink','pf_output'); pulse('set-default-source','pf_unrelated.monitor')
        devices=json.loads(pulse('-f','json','list','sources'))
        assert devices and all(d['name'] in ('pf_output.monitor','pf_unrelated.monitor') for d in devices),devices
        subprocess.run(['cargo','test','--locked','-p','paneferry-audio','private_monitor_shared_opus_playback_and_cleanup',
            '--','--ignored','--nocapture'],cwd=root,env=env,check=True,timeout=90)
        # No capture or playback children should retain an audio stream.
        wait(lambda:json.loads(pulse('-f','json','list','source-outputs'))==[])
        wait(lambda:json.loads(pulse('-f','json','list','sink-inputs'))==[])
        print('PASS: no hardware sources; default source differs from output monitor; all streams cleaned up')
        os.environ.update(env)
        sys.path.insert(0,str(root/'apps/linux-client'))
        from wire import STATE,Wire,request
        import audio
        audio.helper_binary=lambda _:root/'target/debug/paneferry-agent'
        start([str(root/'target/debug/paneferry-agent'),'serve'],'agent')
        wait(lambda:(STATE/'agent.sock').exists())
        directories=[STATE/'network',scratch/'publisher-network']
        for index,path in enumerate(directories):
            start([str(root/'target/debug/paneferry-network'),'serve','--state',str(path),
                '--publisher',str(STATE/'agent.sock'),'--bulk-agent',str(root/'target/debug/paneferry-agent'),'--listen','127.0.0.1:0'],'network-'+str(index))
            wait(lambda:(path/'control.sock').exists())
        def manage(path,**value):
            with socket.socket(socket.AF_UNIX) as client:
                client.settimeout(5);client.connect(str(path/'control.sock'))
                client.sendall(json.dumps(value).encode()+b'\n')
                reply=json.loads(client.makefile('rb').readline())
                assert 'error' not in reply,reply
                return reply
        remote=manage(directories[1],op='status')
        manage(directories[0],op='pair',invitation=manage(directories[1],op='invite')['invitation'],address=remote['listen'])
        peer='quic:'+remote['device_id']
        wait(lambda:(directories[0]/('r-'+remote['device_id']+'.sock')).exists())
        import clipboard_transfer
        clipboard_transfer.helper_binary=lambda _:root/'target/debug/paneferry-network'
        import base64,struct,zlib
        def chunk(name,data):return struct.pack('>I',len(data))+name+data+struct.pack('>I',zlib.crc32(name+data))
        png=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',1,1,8,6,0,0,0))+chunk(b'IDAT',zlib.compress(b'\x00\xff\x00\x00\xff'))+chunk(b'IEND',b'')
        bundles=[dict(kind='image',entries=[dict(path='image.png',size=len(png),directory=False,data=base64.b64encode(png).decode())]),
            dict(kind='files',entries=[dict(path='folder',size=0,directory=True),dict(path='folder/中文.txt',size=3,directory=False,data=base64.b64encode(b'abc').decode())])]
        for bundle in bundles:
            identity=clipboard_transfer.upload(peer,bundle)
            assert clipboard_transfer.download(peer,identity)==bundle
        print('PASS: production GTK binary clipboard image/directory/unicode-file upload and download through paired QUIC')
        wire=Wire(peer); assert audio.CAPABILITY in wire.capabilities
        wire.send(type='audio.subscribe');wire.until('audio.format'); frames=0
        try:
            while True:
                event=wire.read(timeout=3)
                if event['type']=='audio.frame':frames+=1
        except (EOFError,OSError):pass
        finally:wire.close()
        assert frames==16,frames
        wait(lambda:json.loads(pulse('-f','json','list','source-outputs'))==[])
        statuses={}; changes=[]
        def changed(peer,state,text): statuses[peer]=state;changes.append(state)
        service=audio.Service(changed)
        assert service.subscriptions=={}
        service.set(peer,True);subscription=service.subscriptions[peer]
        wait(lambda:statuses.get(peer)=='playing')
        service.set(peer,True);assert service.subscriptions[peer] is subscription
        wait(lambda:len(json.loads(pulse('-f','json','list','source-outputs')))==1)
        wait(lambda:len(json.loads(pulse('-f','json','list','sink-inputs')))==1)
        assert request(peer,'session.list')['sessions']==[], 'audio must work without a video session'
        pulse('set-default-sink','pf_unrelated')
        wait(lambda:'unavailable' in changes)
        wait(lambda:statuses.get(peer)=='playing')
        source_index=next(s['index'] for s in json.loads(pulse('-f','json','list','sources')) if s['name']=='pf_unrelated.monitor')
        assert all(s['source']==source_index for s in json.loads(pulse('-f','json','list','source-outputs')))
        time.sleep(.3)
        request(None,'sharing.set','sharing.changed',local=True,enabled=False)
        wait(lambda:statuses.get(peer)=='unavailable')
        service.set(peer,False);subscription.thread.join(timeout=5)
        assert not subscription.thread.is_alive() and not service.subscriptions
        wait(lambda:json.loads(pulse('-f','json','list','source-outputs'))==[])
        wait(lambda:json.loads(pulse('-f','json','list','sink-inputs'))==[])
        request(None,'sharing.set','sharing.changed',local=True,enabled=True)
        service.set(peer,True); subscription=service.subscriptions[peer]
        wait(lambda:statuses.get(peer)=='playing')
        manage(directories[0],op='revoke',device_id=remote['device_id'])
        wait(lambda:statuses.get(peer)=='unavailable')
        service.stop();subscription.thread.join(timeout=5)
        wait(lambda:json.loads(pulse('-f','json','list','source-outputs'))==[])
        wait(lambda:json.loads(pulse('-f','json','list','sink-inputs'))==[])
        print('PASS: paired QUIC audio, GTK per-device player, default off, duplicate suppression, 16 unacked bound, sharing-off and complete cleanup')
        print('PASS: default-output change reconnects to the new monitor; pairing revocation stops capture and playback')
    except BaseException:
        for log in logs: log.flush()
        for path in scratch.glob('*.log'): print(path.name,path.read_text(errors='replace')[-5000:],file=sys.stderr)
        raise
    finally:
        for process in reversed(processes):
            process.terminate()
            try:process.wait(timeout=5)
            except subprocess.TimeoutExpired:process.kill();process.wait()
        for log in logs:log.close()
