import pathlib,subprocess,json,time,sys,socket,hashlib,os,shlex
p=pathlib.Path('/private/tmp/lra-path-diagnosis');r='/tmp/lra-path-diagnosis';out=p/'results';out.mkdir(exist_ok=True)
ssh=[str(p/'ssh'),'--target','192.168.1.26'];network=p/'target/release/paneferry-network'
label=sys.argv[1] if len(sys.argv)>1 else 'proxy-on';paths=sys.argv[2].split(',') if len(sys.argv)>2 else ['physical','overlay']
manager=subprocess.Popen(ssh+['python3 '+r+'/manager.py'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=(out/(label+'-manager.log')).open('w'),text=True)
ready=json.loads(manager.stdout.readline());expected=hashlib.file_digest((p/'payload.bin').open('rb'),'sha256').hexdigest();size=(p/'payload.bin').stat().st_size
pub=subprocess.Popen([sys.executable,str(p/'publisher.py'),str(p/'pub.sock')]);local=None

def rr(command,**kw):return subprocess.run(ssh+[command],check=True,**kw)
def remote(v):
 manager.stdin.write(json.dumps(v)+'\n');manager.stdin.flush();return json.loads(manager.stdout.readline())
def rpc(state,v):
 with socket.socket(socket.AF_UNIX) as c:
  c.settimeout(30);c.connect(str(state/'control.sock'));c.sendall(json.dumps(v).encode()+b'\n');return json.loads(c.makefile('rb').readline())
try:
 for path in paths:
  host={'physical':'192.168.1.26','overlay':'10.0.1.66'}[path];state=p/(path+'-state')
  local=subprocess.Popen([str(network),'serve','--state',str(state),'--publisher',str(p/'pub.sock'),'--listen','0.0.0.0:0','--bulk-agent',str(p/'bench-file')],stdout=subprocess.DEVNULL,stderr=(out/(label+'-'+path+'-network.log')).open('w'))
  for _ in range(200):
   try:rpc(state,{'op':'status'});break
   except OSError:time.sleep(.05)
  pairing=rpc(state,{'op':'pair','address':host+':'+str(ready['port']),'invitation':remote({'op':'invite'})['invitation']});peer=pairing['device_id']
  subprocess.run([str(p/'ssh'),'--target',host,'true'],check=True)
  for n in range(1,4):
   for transport in (['ssh','quic'] if n%2 else ['quic','ssh']):
    for direction in ['mac-ubuntu','ubuntu-mac']:
     name=f'file-{label}-{path}-{transport}-{direction}-{n}';stop=p/(name+'.stop');stop.unlink(missing_ok=True)
     rr('rm -f '+shlex.quote(r+'/'+name+'.stop'))
     # Warm only the source cache; hashing is outside the timed interval.
     if direction=='mac-ubuntu':
      with (p/'payload.bin').open('rb') as f:
       while f.read(1024*1024):pass
     else:rr('cat '+r+'/payload.bin > /dev/null')
     before={'mac':rpc(state,{'op':'status'}).get('_probe_connections',[]),'ubuntu':remote({'op':'status'}).get('_probe_connections',[])}
     lm=subprocess.Popen([sys.executable,str(p/'monitor.py'),str(out/(name+'-mac.jsonl')),str(stop)])
     rm=subprocess.Popen(ssh+['python3 '+r+'/monitor.py '+r+'/'+name+'-ubuntu.jsonl '+r+'/'+name+'.stop'])
     time.sleep(1.1)
     upload=direction=='mac-ubuntu'
     if transport=='quic':argv=[str(network),'bulk-upload' if upload else 'bulk-download','--state',str(state),'--peer',peer]+([] if upload else ['--transfer','benchmark'])
     else:argv=[str(p/'ssh'),'--target',host,r+'/bench-file '+('clipboard-upload' if upload else 'clipboard-download benchmark')]
     print('START',name,flush=True);start=time.monotonic()
     try:
      if upload:
       with (p/'payload.bin').open('rb') as f:run=subprocess.run(argv,stdin=f,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=180)
       assert run.returncode==0 and run.stdout==b'OK\n',(run.returncode,run.stderr[:200],run.stdout[:100])
      else:
       with (p/'received.bin').open('wb') as f:
        run=subprocess.run(argv,stdin=subprocess.DEVNULL,stdout=f,stderr=subprocess.PIPE,timeout=180)
        f.flush();os.fsync(f.fileno())
       assert run.returncode==0,run.stderr[:200]
      elapsed=time.monotonic()-start
     finally:
      after={'mac':rpc(state,{'op':'status'}).get('_probe_connections',[]),'ubuntu':remote({'op':'status'}).get('_probe_connections',[])}
      time.sleep(1.1);stop.touch();rr('touch '+r+'/'+name+'.stop');lm.wait();rm.wait()
      got=rr('cat '+r+'/'+name+'-ubuntu.jsonl',capture_output=True);(out/(name+'-ubuntu.jsonl')).write_bytes(got.stdout)
     if upload:actual=rr('sha256sum '+r+'/received.bin',capture_output=True,text=True).stdout.split()[0]
     else:actual=hashlib.file_digest((p/'received.bin').open('rb'),'sha256').hexdigest()
     assert actual==expected,'hash mismatch'
     result={'name':name,'bytes':size,'seconds':elapsed,'MB_s':size/elapsed/1e6,'sha256':actual,'compression':False,'streams':1,'includes_fsync':True,'before':before,'after':after,'argv':argv}
     (out/(name+'.json')).write_text(json.dumps(result,indent=2)+'\n');print('DONE',name,round(result['MB_s'],2),'MB/s',flush=True)
  local.terminate();local.wait();local=None
finally:
 if local:local.terminate();local.wait()
 pub.terminate();pub.wait()
 if manager.poll() is None:
  manager.stdin.write('{"op":"stop"}\n');manager.stdin.flush();manager.wait(timeout=10)
