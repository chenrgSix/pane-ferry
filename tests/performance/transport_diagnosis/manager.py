import pathlib,subprocess,sys,time,socket,json
p=pathlib.Path('/tmp/lra-path-diagnosis');state=p/'remote-state'
children=[]
def rpc(v):
 with socket.socket(socket.AF_UNIX) as s:
  s.connect(str(state/'control.sock'));s.sendall(json.dumps(v).encode()+b'\n');return json.loads(s.makefile('rb').readline())
try:
 children.append(subprocess.Popen([sys.executable,str(p/'publisher.py'),str(p/'pub.sock')]))
 children.append(subprocess.Popen([str(p/'target/release/remoteapp-network'),'serve','--state',str(state),'--publisher',str(p/'pub.sock'),'--listen','0.0.0.0:0','--bulk-agent',str(p/'bench-file')],stdout=subprocess.DEVNULL,stderr=(p/'network.log').open('w')))
 for _ in range(200):
  try:rpc({'op':'status'});break
  except OSError:time.sleep(.05)
 status=rpc({'op':'status'});print(json.dumps({'port':int(status['listen'].split(':')[-1]),'pid':children[-1].pid}),flush=True)
 for line in sys.stdin:
  obj=json.loads(line)
  if obj['op']=='stop':break
  print(json.dumps(rpc(obj)),flush=True)
finally:
 for c in children:c.terminate()
 for c in children:c.wait()
