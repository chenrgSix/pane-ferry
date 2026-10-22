#!/usr/bin/env python3
import os,sys,time,json,subprocess,pathlib,re,platform
P=pathlib.Path
linux=platform.system()=='Linux'
def cmd(a):
 return subprocess.run(a,capture_output=True,text=True).stdout

def sample():
 d={'time':time.time(),'monotonic':time.monotonic()}
 if linux:
  d['cpu']=P('/proc/stat').read_text().splitlines()[0]
  d['net']={i:{f:int((P('/sys/class/net')/i/'statistics'/f).read_text()) for f in ['rx_bytes','tx_bytes','rx_packets','tx_packets','rx_errors','tx_errors','rx_dropped','tx_dropped']} for i in ['enp1s0','tun0']}
  d['udp']=P('/proc/net/snmp').read_text();d['disk']=P('/proc/diskstats').read_text()
  threads=[]
  for proc in P('/proc').iterdir():
   if not proc.name.isdigit():continue
   try:
    name=(proc/'comm').read_text().strip()
    if not any(s in name for s in ['iperf3','remoteapp-net','paneferry-net','easytier','ssh','bench-file']):continue
    for th in (proc/'task').iterdir():
     a=(th/'stat').read_text();v=a[a.rfind(')')+2:].split()
     threads.append({'pid':int(proc.name),'tid':int(th.name),'name':name,'cpu_ticks':int(v[11])+int(v[12])})
   except (OSError,ValueError):pass
  d['tcp_sockets']=cmd(['ss','-nt']);d['threads']=threads;d['ticks_per_second']=os.sysconf('SC_CLK_TCK')
 else:
  import ctypes
  lib=ctypes.CDLL('/usr/lib/libSystem.B.dylib');cpu=(ctypes.c_uint*4)();count=ctypes.c_uint(4)
  if lib.host_statistics(lib.mach_host_self(),3,cpu,ctypes.byref(count))==0:d['cpu_ticks']=list(cpu)
  d['net']=cmd(['/usr/sbin/netstat','-ibdn'])
  d['udp']=cmd(['/usr/sbin/netstat','-s','-p','udp'])
  d['disk']=cmd(['/usr/sbin/iostat','-Id'])
  import plistlib
  drives=plistlib.loads(subprocess.run(['/usr/sbin/ioreg','-r','-c','IOBlockStorageDriver','-a'],capture_output=True).stdout)
  d['disk_bytes']={k:sum(x.get('Statistics',{}).get(k,0) for x in drives) for k in ['Bytes (Read)','Bytes (Write)']}
  procs=cmd(['ps','-axo','pid=,comm=']).splitlines()
  pids=[x.strip().split()[0] for x in procs if any(y in x for y in ['lra-path-diagnosis','easytier-gui','/ssh','remoteapp-network','paneferry-network'])]
  d['threads_raw']=cmd(['ps','-M','-p',','.join(pids)]) if pids else ''

 for state in p_states():
  try:
   import socket
   with socket.socket(socket.AF_UNIX) as c:
    c.settimeout(.3);c.connect(str(state/'control.sock'));c.sendall(b'{\"op\":\"status\"}\n');v=json.loads(c.makefile('rb').readline());d.setdefault('quic',[]).extend(v.get('_probe_connections',[]))
  except (OSError,ValueError):pass
 return d
def p_states():
 return list(P('/tmp/lra-path-diagnosis').glob('*-state'))
if __name__=='__main__':
 out=P(sys.argv[1]);stop=P(sys.argv[2])
 with out.open('w') as f:
  while not stop.exists():
   start=time.monotonic();f.write(json.dumps(sample())+'\n');f.flush()
   time.sleep(max(0,1-(time.monotonic()-start)))
