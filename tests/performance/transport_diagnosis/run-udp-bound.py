import pathlib,subprocess,json,time,sys
p=pathlib.Path('/private/tmp/lra-path-diagnosis');r='/tmp/lra-path-diagnosis';ssh=[str(p/'ssh'),'--target','192.168.1.26'];out=p/'results';out.mkdir(exist_ok=True)
subprocess.run(ssh+['cat > '+r+'/monitor.py'],input=(p/'monitor.py').read_bytes(),check=True)
server=subprocess.Popen(ssh+[r+'/iperf-3.22/src/iperf3 -s -B 192.168.1.26 -p 45201'],stdout=(out/'iperf-server.log').open('w'),stderr=subprocess.STDOUT)
time.sleep(1)
try:
 for n in range(1,4):
  for path,host in [('physical','192.168.1.26')]:
   for reverse in [False,True]:
    name=f'iperf-udp1200-bound-{path}-'+('ubuntu-mac' if reverse else 'mac-ubuntu')+f'-{n}'
    stop=p/(name+'.stop');stop.unlink(missing_ok=True)
    local=subprocess.Popen([sys.executable,str(p/'monitor.py'),str(out/(name+'-mac.jsonl')),str(stop)])
    remote=subprocess.Popen(ssh+['python3 '+r+'/monitor.py '+r+'/'+name+'-ubuntu.jsonl '+r+'/'+name+'.stop'])
    args=[str(p/'iperf-3.22/src/iperf3'),'-c',host,'-p','45201','-P','1','-t','10','-u','-b','100M','-l','1200','-J','--get-server-output']+(['-R'] if reverse else [])
    print('START',name,flush=True)
    run=subprocess.run(args,capture_output=True,text=True,timeout=50)
    (out/(name+'.json')).write_text(run.stdout);(out/(name+'.err')).write_text(run.stderr)
    stop.touch();subprocess.run(ssh+['touch '+r+'/'+name+'.stop'],check=True);local.wait();remote.wait()
    got=subprocess.run(ssh+['cat '+r+'/'+name+'-ubuntu.jsonl'],capture_output=True,check=True)
    (out/(name+'-ubuntu.jsonl')).write_bytes(got.stdout)
    j=json.loads(run.stdout)
    print('DONE',name,json.dumps(j.get('end',{}).get('sum',{})),j.get('error',''),flush=True)
finally:
 server.terminate();server.wait()
 subprocess.run(ssh+["pkill -f '^/tmp/lra-path-diagnosis/iperf-3.22/src/iperf3 -s -B 192.168.1.26 -p 45201$'"],capture_output=True)
