import pathlib,json,re,statistics,collections
P=pathlib.Path('/private/tmp/lra-path-diagnosis/results')
def linux_udp(s):
 lines=s.splitlines()
 for a,b in zip(lines,lines[1:]):
  if a.startswith('Udp:') and b.startswith('Udp:'):return dict(zip(a.split()[1:],map(int,b.split()[1:])))
 return {}
def mac_net(s):
 d={}
 for l in s.splitlines():
  v=l.split()
  if len(v)>10 and v[0] in ['en0','utun4'] and '<Link#' in l:d[v[0]]=dict(zip(['rx_packets','rx_errors','rx_bytes','tx_packets','tx_errors','tx_bytes','collisions','drop'],map(int,v[-8:])))
 return d
def mac_cpu(s):
 d=[];lastpid=None;name='';i=0
 for l in s.splitlines()[1:]:
  v=l.split();offset=0
  if not v:continue
  if not v[0].isdigit():
   if len(v)<9:continue
   pid=int(v[1]);i=0;name=v[8];cpu=float(v[3]);st=v[6];ut=v[7]
  else:
   if len(v)<6:continue
   pid=int(v[0]);cpu=float(v[1]);st=v[4];ut=v[5]
  d.append({'pid':pid,'index':i,'cpu':cpu,'name':name});i+=1
 return d

def summarize_samples(path,linux):
 ss=[json.loads(l) for l in path.read_text().splitlines()];a,b=ss[0],ss[-1];dt=b['monotonic']-a['monotonic'];d={'samples':len(ss),'span_s':dt}
 if linux:
  d['net_delta']={i:{k:b['net'][i][k]-v for k,v in x.items()} for i,x in a['net'].items()}
  ua,ub=linux_udp(a['udp']),linux_udp(b['udp']);d['udp_delta']={k:ub[k]-v for k,v in ua.items()}
  peak=collections.defaultdict(float);sums=collections.defaultdict(list)
  for x,y in zip(ss,ss[1:]):
   old={(t['pid'],t['tid']):t for t in x['threads']};proc=collections.defaultdict(float);interval=y['monotonic']-x['monotonic']
   for t in y['threads']:
    before=old.get((t['pid'],t['tid']))
    if before:
     cpu=(t['cpu_ticks']-before['cpu_ticks'])/y['ticks_per_second']/interval*100;peak[t['name']]=max(peak[t['name']],cpu);proc[t['name']]+=cpu
   for k,v in proc.items():sums[k].append(v)
  d['cpu']={k:{'peak_thread_pct':v,'mean_sum_threads_pct':statistics.mean(sums[k])} for k,v in peak.items()}
  def disk(s):
   v=next((l.split() for l in s.splitlines() if l.split()[2]=='nvme0n1'),None)
   return {'read_bytes':int(v[5])*512,'write_bytes':int(v[9])*512,'busy_ms':int(v[12])} if v else {}
  da,db=disk(a['disk']),disk(b['disk']);d['disk_delta']={k:db[k]-v for k,v in da.items()}
  ca=list(map(int,a['cpu'].split()[1:]));cb=list(map(int,b['cpu'].split()[1:]));delta=[y-x for x,y in zip(ca,cb)];total=sum(delta[:8]);d['system_cpu_busy_pct']=100*(total-delta[3]-delta[4])/total if total else 0
 else:
  na,nb=mac_net(a['net']),mac_net(b['net']);d['net_delta']={i:{k:nb[i][k]-v for k,v in x.items()} for i,x in na.items()}
  def udp(s):return {m[1]:int(m[0]) for m in re.findall(r'^\s*(\d+) (dropped due to [^\n]+|with bad checksum|datagrams received|datagrams output)',s,re.M)}
  ua,ub=udp(a['udp']),udp(b['udp']);d['udp_delta']={k:ub[k]-v for k,v in ua.items()}
  peaks=collections.defaultdict(float);sums=collections.defaultdict(list)
  for s in ss:
   cur=collections.defaultdict(float)
   for t in mac_cpu(s['threads_raw']):
    name=pathlib.Path(t['name']).name.rstrip(':')
    if name not in ['iperf3','easytier-gui','remoteapp-network','paneferry-network','ssh','bench-file']:continue
    peaks[name]=max(peaks[name],t['cpu']);cur[name]+=t['cpu']
   for k,v in cur.items():sums[k].append(v)
  d['cpu']={k:{'peak_thread_pct':v,'mean_sum_threads_pct':statistics.mean(sums[k])} for k,v in peaks.items()}
  try:d['disk_delta']={'aggregate_io_MiB':float(b['disk'].split()[-1])-float(a['disk'].split()[-1])}
  except ValueError:d['disk_delta']={}
  if 'disk_bytes' in a and 'disk_bytes' in b:d['disk_delta'].update({k:b['disk_bytes'][k]-v for k,v in a['disk_bytes'].items()})
  if 'cpu_ticks' in a and 'cpu_ticks' in b:
   ticks=[y-x for x,y in zip(a['cpu_ticks'],b['cpu_ticks'])];total=sum(ticks);d['system_cpu_busy_pct']=100*(total-ticks[2])/total if total else 0
 wanted=('10.0.1.253' if linux else '10.0.1.66') if 'overlay' in path.name else ('192.168.1.80' if linux else '192.168.1.26')
 quic=[q for s in ss for q in s.get('quic',[]) if q['remote'].split(':')[0]==wanted]
 if quic:d['quic_samples']={'rtt_ms_min':min(q['rtt_ms'] for q in quic),'rtt_ms_max':max(q['rtt_ms'] for q in quic),'cwnd_min':min(q['cwnd'] for q in quic),'cwnd_max':max(q['cwnd'] for q in quic),'mtu':sorted(set(q['mtu'] for q in quic))}
 return d
rows=[]
for f in sorted(P.glob('*.json')):
 if not f.name.startswith(('file-','iperf-')):continue
 j=json.loads(f.read_text());name=f.stem
 if j.get('error'):continue
 d={'name':name,'MB_s':j.get('MB_s',j.get('end',{}).get('sum_received',{}).get('bits_per_second',0)/8e6)}
 for system in ['mac','ubuntu']:
  sf=f.with_name(name+'-'+system+'.jsonl')
  if sf.exists():d[system]=summarize_samples(sf,system=='ubuntu')
 if 'before'in j:
  d['quic_before_after']={}
  for system in ['mac','ubuntu']:
   if j['before'][system] and j['after'][system]:
    aa={c['remote']:c for c in j['before'][system]};bb={c['remote']:c for c in j['after'][system]};key=max(aa.keys()&bb.keys(),key=lambda k:bb[k]['tx_bytes']-aa[k]['tx_bytes']+bb[k]['rx_bytes']-aa[k]['rx_bytes']);a,b=aa[key],bb[key]
    d['quic_before_after'][system]={k:b[k]-v for k,v in a.items() if k not in ['remote','rtt_ms','cwnd','mtu']}
 rows.append(d)
(P/'summary.json').write_text(json.dumps(rows,indent=2)+'\n')
for d in rows:
 print(d['name'],round(d['MB_s'],2), 'CPU',[(s,{k:round(v['peak_thread_pct'],1) for k,v in d.get(s,{}).get('cpu',{}).items()}) for s in ['mac','ubuntu']], 'UDP',[(s,d.get(s,{}).get('udp_delta')) for s in ['mac','ubuntu']])
