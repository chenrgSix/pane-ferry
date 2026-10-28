#!/usr/bin/env python3
"""Build-source instrumentation for X11 readback -> local socket write, without pixels.

Creates a git-archive copy. Never edits or runs the installed Agent. Timings stay
in PANEFERRY_PUBLISHER_TRACE (0600), without adding fields to the wire protocol.
Use only with a test-owned window. Capture begins at readback entry, after the
worker's initial ownership check; it excludes application rendering and sampling
admission. Each capture has a unique ID carried through queues to socket write.
"""
import argparse
from pathlib import Path

from agent_stage_probe import instrument


BACKEND = r'''
use serde_json::{json, Value};
use std::{collections::HashMap, sync::{Mutex, OnceLock, atomic::{AtomicU64, Ordering}}};
pub fn now() -> u64 {
    let mut t=libc::timespec{tv_sec:0,tv_nsec:0};
    assert_eq!(unsafe{libc::clock_gettime(libc::CLOCK_MONOTONIC,&mut t)},0);
    t.tv_sec as u64*1_000_000_000+t.tv_nsec as u64
}
static FRAMES:OnceLock<Mutex<HashMap<usize,Value>>>=OnceLock::new();
pub fn captured(key:usize,start:u64,backend:&str) {
    let end=now();
    static NEXT:AtomicU64=AtomicU64::new(1);
    let id=NEXT.fetch_add(1,Ordering::Relaxed);
    let mut frames=FRAMES.get_or_init(Default::default).lock().unwrap();
    if frames.len()>1024 {frames.clear();}
    frames.insert(key,json!({"capture_id":id,"capture_start_ns":start,"capture_end_ns":end,"capture_backend":backend}));
}
pub fn take(key:usize)->Value {
    FRAMES.get_or_init(Default::default).lock().unwrap().remove(&key).unwrap_or(Value::Null)
}
'''

AGENT = r'''
pub use paneferry_backend_x11::publisher_timing::now;
use serde_json::{json,Value};
use std::{collections::HashMap, fs::{File,OpenOptions}, io::Write,
    os::unix::fs::OpenOptionsExt, sync::{Mutex,OnceLock}};
static FRAMES:OnceLock<Mutex<HashMap<usize,Value>>>=OnceLock::new();
pub fn published(key:usize,mut value:Value) {
    if !value.is_object(){return;}
    value["publish_ns"]=json!(now());
    let mut frames=FRAMES.get_or_init(Default::default).lock().unwrap();
    if frames.len()>1024{frames.clear();}
    frames.insert(key,value);
}
pub fn encoding(key:usize)->Value {
    let entered=now();
    let mut value=FRAMES.get_or_init(Default::default).lock().unwrap().remove(&key).unwrap_or(Value::Null);
    if value.is_object(){value["encode_enter_ns"]=json!(entered);}
    value
}
pub fn sent(mut value:Value,sequence:u64,serialize:u64,write:u64,end:u64,bytes:usize) {
    if !value.is_object(){return;}
    value["sequence"]=json!(sequence);value["serialize_start_ns"]=json!(serialize);
    value["write_begin_ns"]=json!(write);value["write_end_ns"]=json!(end);value["json_bytes"]=json!(bytes);
    static FILE:OnceLock<Mutex<Option<File>>>=OnceLock::new();
    let file=FILE.get_or_init(||Mutex::new(std::env::var_os("PANEFERRY_PUBLISHER_TRACE")
        .map(|path|OpenOptions::new().append(true).create(true).mode(0o600).open(path).unwrap())));
    if let Some(file)=file.lock().unwrap().as_mut(){
        let mut bytes=serde_json::to_vec(&value).unwrap();bytes.push(b'\n');file.write_all(&bytes).unwrap();
    }
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('destination', type=Path)
    parser.add_argument('--revision', default='HEAD')
    parser.add_argument('--source-override', action='append', default=[])
    args = parser.parse_args()
    destination = args.destination.resolve()
    instrument(destination, args.revision, stages=False, source_overrides=args.source_override)

    def edit(name, before, after):
        path = destination / name
        text = path.read_text()
        if text.count(before) != 1:
            raise ValueError(f'publisher timing anchor changed in {name}: {before[:60]}')
        path.write_text(text.replace(before, after))

    backend = 'crates/backend-x11/src/'
    agent = 'apps/linux-agent/src/'
    (destination / backend / 'publisher_timing.rs').write_text(BACKEND)
    (destination / agent / 'publisher_timing.rs').write_text(AGENT)
    edit(backend + 'lib.rs', 'mod readback;', 'mod readback;\npub mod publisher_timing;')
    edit(backend + 'readback.rs', '    let pixmap = if surface.composite {',
         '    let probe_start=crate::publisher_timing::now();\n    let mut probe_backend="socket";\n    let pixmap = if surface.composite {')
    edit(backend + 'readback.rs', '            return Ok(mapped);',
         '            probe_backend="linear";\n            return Ok(mapped);')
    edit(backend + 'readback.rs', '            return Ok(image);',
         '            probe_backend="shm";\n            return Ok(image);')
    edit(backend + 'readback.rs', '    Ok(CapturedImage {', '    let image=CapturedImage {')
    edit(backend + 'readback.rs', '    })\n}',
         '    };\n    crate::publisher_timing::captured(image.data.as_ptr() as usize,probe_start,probe_backend);\n    Ok(image)\n}')
    edit(agent + 'main.rs', 'static STOP:', 'mod publisher_timing;\nstatic STOP:')
    edit(agent + 'video_frame.rs', '    pub fn pixels(&self) -> usize {', '''    pub fn publisher_timing(&self)->serde_json::Value {
        match &self.storage {
            Storage::Image(image)=>paneferry_backend_x11::publisher_timing::take(image.data.as_ptr() as usize),
            _=>serde_json::Value::Null
        }
    }
    pub fn pixels(&self) -> usize {''')
    edit(agent + 'output.rs', '        self.push(Pending::Frame(window.clone(), image, session.into()))',
         '        crate::publisher_timing::published(Arc::as_ptr(&image) as usize,image.publisher_timing());\n'
         '        self.push(Pending::Frame(window.clone(), image, session.into()))')
    edit(agent + 'output_worker.rs', '        let id = job.window.window_id;',
         '        let mut probe=crate::publisher_timing::encoding(Arc::as_ptr(&job.image) as usize);\n'
         '        let id = job.window.window_id;')
    edit(agent + 'output_worker.rs', '        let packet = state.encoder.encode_frame(&job.image)?;',
         '        if probe.is_object(){probe["encode_call_start_ns"]=json!(crate::publisher_timing::now());}\n'
         '        let packet = state.encoder.encode_frame(&job.image)?;\n'
         '        if probe.is_object(){probe["encode_end_ns"]=json!(crate::publisher_timing::now());}')
    edit(agent + 'output_worker.rs', '"width":job.window.width, "height":job.window.height, "codec":"h264",',
         '"_publisher_stages":probe, "width":job.window.width, "height":job.window.height, "codec":"h264",')
    edit(agent + 'output.rs', '        let mut bytes = serde_json::to_vec(&value)?;',
         '        let probe=value.as_object_mut().and_then(|v|v.remove("_publisher_stages")).unwrap_or(Value::Null);\n'
         '        let probe_serialize=crate::publisher_timing::now();\n'
         '        let mut bytes = serde_json::to_vec(&value)?;')
    edit(agent + 'output.rs', '        socket.write_all(&bytes)?;',
         '        let probe_write=crate::publisher_timing::now();\n'
         '        socket.write_all(&bytes)?;\n'
         '        crate::publisher_timing::sent(probe,sequence,probe_serialize,probe_write,crate::publisher_timing::now(),bytes.len());')


if __name__ == '__main__':
    main()
