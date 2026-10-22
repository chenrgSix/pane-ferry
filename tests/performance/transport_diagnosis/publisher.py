import socket,json,threading,sys,pathlib
p=pathlib.Path(sys.argv[1]);p.unlink(missing_ok=True);s=socket.socket(socket.AF_UNIX);s.bind(str(p));s.listen()
def client(c):
 with c,c.makefile('rb') as f:
  for line in f:
   x=json.loads(line);c.sendall(json.dumps({'version':1,'type':x['type'],'sharing_enabled':True}).encode()+b'\n')
while True:
 c,_=s.accept();threading.Thread(target=client,args=(c,),daemon=True).start()
