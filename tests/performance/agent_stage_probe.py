#!/usr/bin/env python3
"""Instrument a fresh git-archive copy, never the working tree or installed Agent.

Only diagnostic input/fixture counts and timestamps are added to frame metadata. Run one
sequential key/click probe/controller; do not use this binary in production.
"""
import argparse
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
MODULE = r'''
use std::{cell::Cell, collections::HashMap, sync::{Mutex, OnceLock}};
use serde_json::{json, Value};
use paneferry_protocol::Request;
pub fn now() -> u64 {
    let mut t = libc::timespec {tv_sec:0,tv_nsec:0};
    assert_eq!(unsafe {libc::clock_gettime(libc::CLOCK_MONOTONIC,&mut t)},0);
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}
#[derive(Default,Clone)]
struct Input { count:u64, received:u64, dispatched:u64, input_end:u64 }
#[derive(Default)]
struct State { inputs:HashMap<u32,Input>, active:Option<u32>, sinks:HashMap<usize,bool>, raw_first:u64, raw_last:u64, converted_first:u64, converted_last:u64, pull_first:u64, raw_matching:u64, converted_matching:u64, frames:HashMap<usize,Value> }
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
thread_local! { static WINDOW:Cell<Option<u32>>=const { Cell::new(None) }; static INTERVAL:Cell<u64>=const { Cell::new(0) }; static SCAN_START:Cell<u64>=const {Cell::new(0)}; static SCAN_END:Cell<u64>=const {Cell::new(0)}; }
pub fn is_input(r:&Request)->bool { window(r).is_some() }
fn window(r:&Request)->Option<u32> {
    match r {
        Request::Pointer{window_id,button:1|3,pressed:true,..} |
        Request::Key{window_id,keysym:0xff51,pressed:true} => Some(*window_id),
        _=>None
    }
}
pub fn request(stage:&str,r:&Request) {
    let Some(id)=window(r) else{return};
    let mut s=STATE.get_or_init(Default::default).lock().unwrap();
    if stage=="received" {
        if s.inputs.len()>64{s.inputs.clear();}
        let input=s.inputs.entry(id).or_default();
        input.count+=1;input.received=now();input.dispatched=0;input.input_end=0;
        s.active=Some(id);s.raw_first=0;s.raw_last=0;s.converted_first=0;s.converted_last=0;s.pull_first=0;s.raw_matching=0;s.converted_matching=0;
    } else if stage=="dispatched" {
        WINDOW.set(Some(id));s.inputs.entry(id).or_default().dispatched=now();
    }
}
pub fn input_end() {
    if let Some(id)=WINDOW.get(){STATE.get_or_init(Default::default).lock().unwrap().inputs.entry(id).or_default().input_end=now();}
}
pub fn sink(pointer:usize,raw:bool) {STATE.get_or_init(Default::default).lock().unwrap().sinks.insert(pointer,raw);}
pub fn wake(pointer:usize) {
 let mut s=STATE.get_or_init(Default::default).lock().unwrap();let t=now();
 if s.active.is_none(){return;}
 match s.sinks.get(&pointer).copied() {
 Some(true)=>{if s.raw_first==0{s.raw_first=t;}s.raw_last=t;},
 Some(false)=>{if s.converted_first==0{s.converted_first=t;}s.converted_last=t;},_=>{}
 }
}
pub fn pulling(){let mut s=STATE.get_or_init(Default::default).lock().unwrap();if s.active.is_some() && s.pull_first==0{s.pull_first=now();}}
pub fn pixels(raw:bool,marker:u64) {
    let mut s=STATE.get_or_init(Default::default).lock().unwrap();
    if !s.active.is_some_and(|id|s.inputs.get(&id).is_some_and(|i|i.count==marker)){return;}
    let t=now();
    if raw && s.raw_matching==0{s.raw_matching=t;}
    if !raw && s.converted_matching==0{s.converted_matching=t;}
}
pub fn interval(n:u64){INTERVAL.set(n);}
pub fn scan(start:bool){if start{SCAN_START.set(now());}else{SCAN_END.set(now());}}
pub fn publish(pointer:usize) {
    let mut s=STATE.get_or_init(Default::default).lock().unwrap();
    if let Some(v)=s.frames.get_mut(&pointer){v["publish"]=json!(now());v["scan_start"]=json!(SCAN_START.get());v["scan_end"]=json!(SCAN_END.get());}
}
pub fn capture(window:u32,pointer:usize,start:u64) {
    let mut s=STATE.get_or_init(Default::default).lock().unwrap();
    let Some(i)=s.inputs.get(&window).cloned() else{return};
    let v=json!({"count":i.count,"received":i.received,"dispatched":i.dispatched,
        "input_end":i.input_end,"capture_start":start,"capture_end":now(),"raw_first":s.raw_first,"raw_last":s.raw_last,"converted_first":s.converted_first,"converted_last":s.converted_last,"pull_first":s.pull_first,"raw_matching":s.raw_matching,"converted_matching":s.converted_matching,"interval":INTERVAL.get()});
    if s.frames.len()>128 {s.frames.clear();}
    s.frames.insert(pointer,v);
}
pub fn frame(pointer:usize,start:u64)->Value {
    let mut v=STATE.get_or_init(Default::default).lock().unwrap().frames.get(&pointer).cloned().unwrap_or(Value::Null);
    if v.is_object() {v["encode_start"]=json!(start);v["encode_end"]=json!(now());}
    v
}
'''

def instrument(destination, revision):
    destination.mkdir(parents=True, exist_ok=False)
    with tempfile.TemporaryFile() as archive:
        subprocess.run(['git', 'archive', revision], cwd=ROOT, stdout=archive, check=True)
        archive.seek(0)
        subprocess.run(['tar', '-xf', '-', '-C', str(destination)], stdin=archive, check=True)
    source = destination / 'apps/linux-agent/src'
    def edit(name, before, after):
        p = source / name
        text = p.read_text()
        if text.count(before) != 1:
            raise ValueError(f'instrumentation anchor changed in {name}: {before[:80]}')
        p.write_text(text.replace(before, after))
    (source / 'stage_probe.rs').write_text(MODULE)
    edit('main.rs', 'static STOP:', 'mod stage_probe;\nstatic STOP:')
    edit('main.rs', 'if tx.send(req).is_err() {', 'stage_probe::request("received", &req);\n                            if tx.send(req).is_err() {')
    edit('main.rs', 'if triggers_fast_scan(&req) {', 'let probe_input = stage_probe::is_input(&req);\n            stage_probe::request("dispatched", &req);\n            if triggers_fast_scan(&req) {')
    edit('main.rs', 'if let Err(e) = result {', 'if probe_input {stage_probe::input_end();}\n            if let Err(e) = result {')
    edit('video.rs', 'let signal = connect_wake(api, sink, wake)?;', 'crate::stage_probe::sink(sink as usize,true);\n        let signal = connect_wake(api, sink, wake)?;')
    edit('video_frame.rs', 'self.signal = connect_wake(self.api, self.sink, wake)?;', 'crate::stage_probe::sink(self.sink as usize,false);\n        self.signal = connect_wake(self.api, self.sink, wake)?;')
    edit('video.rs', 'let wake = unsafe { &*data.cast::<Arc<FrameWake>>() };', 'crate::stage_probe::wake(_sink as usize);\n    let wake = unsafe { &*data.cast::<Arc<FrameWake>>() };')
    edit('video.rs', 'let sample = unsafe { (self.api.pull)(self.sink, 0) };', 'crate::stage_probe::pulling();\n        let sample = unsafe { (self.api.pull)(self.sink, 0) };')
    edit('video.rs', 'converter.submit(buffer)?;', '''if spec.region[2]>=256 && spec.region[3]>16 {
                let mapping=ReadMap::new(self.api,buffer)?;
                let bytes=mapping.bytes()?;
                let mut marker=0u64;
                for bit in 0..8 {let offset=((spec.region[1]+16) as usize*spec.source_width as usize+spec.region[0] as usize+16+bit*32)*4;if bytes.get(offset).is_some_and(|v|*v>128){marker|=1<<bit;}}
                crate::stage_probe::pixels(true,marker);
            }
            converter.submit(buffer)?;''')
    edit('video_frame.rs', 'Ok(Some(Frame {', '''if width>=256 && height>16 {
            let mapping=ReadMap::new(self.api,output)?;
            let bytes=mapping.bytes()?;
            let mut marker=0u64;
            for bit in 0..8 {let offset=16*w.next_multiple_of(4)+16+bit*32;if bytes.get(offset).is_some_and(|v|*v>128){marker|=1<<bit;}}
            crate::stage_probe::pixels(false,marker);
        }
        Ok(Some(Frame {''')
    edit('main.rs', 'let capture_due = capture_pending && last_capture.elapsed() >= frame_interval;', 'stage_probe::interval(frame_interval.as_nanos() as u64);\n        let capture_due = capture_pending && last_capture.elapsed() >= frame_interval;')
    edit('main.rs', 'let mut windows = if !s.has_exited()? && scan_due {\n                match s.scan_windows() {', 'let mut windows = if !s.has_exited()? && scan_due {\n                stage_probe::scan(true);\n                let probe_scan_result=s.scan_windows();\n                stage_probe::scan(false);\n                match probe_scan_result {')
    edit('main.rs', 'out.frame(w, image, &sid)?;', 'stage_probe::publish(Arc::as_ptr(&image) as usize);\n                        out.frame(w, image, &sid)?;')
    edit('wayland.rs', 'let result = if self.scene.is_some() {', 'let probe_capture = crate::stage_probe::now();\n            let result = if self.scene.is_some() {')
    edit('wayland.rs', 'stream.image = Some(Arc::new(image));', 'let image = Arc::new(image);\n                crate::stage_probe::capture(*id, Arc::as_ptr(&image) as usize, probe_capture);\n                stream.image = Some(image);')
    edit('output_worker.rs', 'let packet = state.encoder.encode_frame(&job.image)?;', 'let probe_encode = crate::stage_probe::now();\n        let packet = state.encoder.encode_frame(&job.image)?;')
    edit('output_worker.rs', '"stream_id":state.encoder.stream,', '"_agent_stages":crate::stage_probe::frame(Arc::as_ptr(&job.image) as usize,probe_encode),\n            "stream_id":state.encoder.stream,')
    edit('output.rs', 'let mut bytes = serde_json::to_vec(&value)?;', 'if value["_agent_stages"].is_object() {value["_agent_stages"]["write"]=json!(crate::stage_probe::now());}\n        let mut bytes = serde_json::to_vec(&value)?;')
    print(destination)

if __name__ == '__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('destination', type=Path)
    p.add_argument('--revision', default='HEAD')
    a=p.parse_args()
    instrument(a.destination.resolve(), a.revision)
