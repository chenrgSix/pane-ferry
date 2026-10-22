from pathlib import Path
import shutil
root=Path('/Users/chenzhirong/Code/ideProject/lan-remoteapp');dst=Path('/private/tmp/lra-path-diagnosis/source')
dst.mkdir(exist_ok=True)
for name in ['Cargo.toml','Cargo.lock']:shutil.copy2(root/name,dst/name)
for name in ['crates','apps/linux-agent']:
 shutil.copytree(root/name,dst/name,dirs_exist_ok=True,ignore=shutil.ignore_patterns('target','.build'))
f=dst/'crates/network/src/service.rs';s=f.read_text();anchor='"peers":peers})';assert s.count(anchor)==1
s=s.replace(anchor,'''"peers":peers,"_probe_connections":state.connections.values().flatten().map(|c| {
 let s=c.stats(); json!({"remote":c.remote_address().to_string(),"rtt_ms":s.path.rtt.as_secs_f64()*1000.0,"cwnd":s.path.cwnd,
 "sent_packets":s.path.sent_packets,"lost_packets":s.path.lost_packets,"congestion_events":s.path.congestion_events,"mtu":s.path.current_mtu,
 "tx_bytes":s.udp_tx.bytes,"rx_bytes":s.udp_rx.bytes,"data_blocked_tx":s.frame_tx.data_blocked,"data_blocked_rx":s.frame_rx.data_blocked,
 "stream_blocked_tx":s.frame_tx.stream_data_blocked,"stream_blocked_rx":s.frame_rx.stream_data_blocked})
 }).collect::<Vec<_>>()})''');f.write_text(s)
