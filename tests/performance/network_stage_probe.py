#!/usr/bin/env python3
"""Instrument a git-archive network helper without changing protocol metadata.

Set PANEFERRY_NETWORK_PROBE_TRACE to a private path prefix. Per-process JSONL
files contain frame IDs, lengths and timestamps only; never frame payloads.
macOS timestamps use CLOCK_UPTIME_RAW, matching the WindowHost uptime clock.
"""
import argparse
from pathlib import Path
from agent_stage_probe import ROOT, instrument

def build_archive(destination, revision):
    instrument(destination, revision, stages=False)
    source = destination / 'crates/network/src'
    def edit(name, before, after):
        p = source / name
        s = p.read_text()
        if s.count(before) != 1:
            raise ValueError(f'network probe anchor changed: {name}: {before[:80]}')
        p.write_text(s.replace(before, after))
    (source / 'probe.rs').write_bytes((ROOT / 'tests/performance/network_probe.rs').read_bytes())
    edit('lib.rs', 'mod diagnostics;', 'mod probe;\nmod diagnostics;')
    edit('framing.rs', 'pub async fn json<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> Result<()> {',
         'pub async fn json<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> Result<()> {\n    let probe_begin=crate::probe::now();')
    edit('framing.rs', "bytes.push(b'\\n');\n    writer.write_all(&bytes).await?;\n    writer.flush().await?;",
         "bytes.push(b'\\n');\n    let probe_serialized=crate::probe::now();\n    writer.write_all(&bytes).await?;\n    writer.flush().await?;\n    crate::probe::frame(\"json_write_complete\",value,crate::probe::now(),serde_json::json!({\"serialize_begin_ns\":probe_begin,\"serialize_end_ns\":probe_serialized,\"json_line_bytes\":bytes.len()}));")
    edit('framing.rs', 'pub fn new(mut value: Value, barrier: u64) -> Result<Self> {',
         'pub fn new(mut value: Value, barrier: u64) -> Result<Self> {\n        let probe_begin=crate::probe::now();')
    edit('framing.rs', '        Ok(Self {\n            value,\n            payload,\n            barrier,',
         '        crate::probe::frame("packet_assembled",&value,crate::probe::now(),serde_json::json!({"assemble_begin_ns":probe_begin,"binary_bytes":payload.len()}));\n        Ok(Self {\n            value,\n            payload,\n            barrier,')
    edit('framing.rs', 'pub fn restore(mut self) -> Value {',
         'pub fn restore(mut self) -> Value {\n        let probe_begin=crate::probe::now();\n        let probe_binary=self.payload.len();')
    edit('framing.rs', '        self.value\n    }\n    pub async fn write',
         '        crate::probe::frame("packet_restored",&self.value,crate::probe::now(),serde_json::json!({"restore_begin_ns":probe_begin,"binary_bytes":probe_binary}));\n        self.value\n    }\n    pub async fn write')
    edit('framing.rs', 'pub async fn write<W: AsyncWrite + Unpin>(&self, writer: &mut W) -> Result<()> {',
         'pub async fn write<W: AsyncWrite + Unpin>(&self, writer: &mut W) -> Result<()> {\n        let probe_begin=crate::probe::now();')
    edit('framing.rs', '        writer.write_u32(meta.len() as u32).await?;',
         '        crate::probe::frame("quic_write_begin",&self.value,probe_begin,serde_json::json!({"binary_bytes":self.payload.len(),"metadata_bytes":meta.len()}));\n        writer.write_u32(meta.len() as u32).await?;')
    edit('framing.rs', '        writer.flush().await?;\n        Ok(())\n    }\n    pub async fn read',
         '        writer.flush().await?;\n        crate::probe::frame("quic_write_complete",&self.value,crate::probe::now(),serde_json::json!({"write_begin_ns":probe_begin,"binary_bytes":self.payload.len(),"metadata_bytes":meta.len()}));\n        Ok(())\n    }\n    pub async fn read')
    edit('framing.rs', '        // Idle streams are valid. Once a packet begins,',
         '        let probe_first=crate::probe::now();\n        // Idle streams are valid. Once a packet begins,')
    edit('framing.rs', '            reader.read_exact(&mut payload).await?;',
         '            reader.read_exact(&mut payload).await?;\n            crate::probe::frame("quic_read_complete",&value,crate::probe::now(),serde_json::json!({"first_byte_ns":probe_first,"binary_bytes":binary,"metadata_bytes":meta}));')
    edit('service.rs', 'within(Instant::now()+TIMEOUT, "media header", Packet::new(json!({"op":"media","channel":channel,"window_id":window}),0)?.write(&mut send)).await?;',
         'let header=Packet::new(json!({"op":"media","channel":channel,"window_id":window}),0)?;\n let mut probe_offset=16+serde_json::to_vec(&header.value)?.len() as u64;\n within(Instant::now()+TIMEOUT,"media header",header.write(&mut send)).await?;')
    edit('service.rs', 'while let Some(packet) = rx.recv().await { tokio::time::timeout(TIMEOUT,packet.write(&mut send)).await??; }',
         'while let Some(packet) = rx.recv().await {\n crate::probe::progress(connection.clone(),&packet.value,u64::from(send.id()),probe_offset);\n probe_offset+=16+serde_json::to_vec(&packet.value)?.len() as u64+packet.payload.len() as u64;\n tokio::time::timeout(TIMEOUT,packet.write(&mut send)).await??; }')

if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('destination', type=Path)
    p.add_argument('--revision', default='HEAD')
    a = p.parse_args()
    build_archive(a.destination.resolve(), a.revision)
