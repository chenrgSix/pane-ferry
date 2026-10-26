#!/usr/bin/env python3
"""Instrument a fresh git-archive copy, never the working tree or installed Agent.

Only diagnostic frame identities, fixture counts and timestamps are collected.
By default they extend frame metadata; --trace-only keeps the wire metadata
unchanged and stores timings in the private PANEFERRY_PROBE_TRACE file instead.
Run one sequential key/click probe/controller; do not use this binary in production.
"""
import argparse
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
MODULE = r'''
use std::{cell::Cell, collections::HashMap, sync::{Mutex, OnceLock, atomic::{AtomicU64, Ordering}}};
use serde_json::{json, Value};
use paneferry_protocol::Request;
pub fn now() -> u64 {
    let mut t = libc::timespec {tv_sec:0,tv_nsec:0};
    assert_eq!(unsafe {libc::clock_gettime(libc::CLOCK_MONOTONIC,&mut t)},0);
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}
pub fn rpc_queued(role:&str,op:&str)->u64 {
    static NEXT:AtomicU64=AtomicU64::new(1);
    let id=NEXT.fetch_add(1,Ordering::Relaxed);
    let s=STATE.get_or_init(Default::default).lock().unwrap();
    let count=s.active.and_then(|w|s.inputs.get(&w)).map(|i|i.count);
    drop(s);
    crate::selection_probe::trace(json!({"event":"rpc_queued","rpc_id":id,"role":role,"op":op,"count":count,"timestamp_ns":now()}));
    id
}
pub fn rpc_event(id:u64,event:&str) {
    crate::selection_probe::trace(json!({"event":event,"rpc_id":id,"timestamp_ns":now()}));
}
#[derive(Default,Clone)]
struct Input { count:u64, received:u64, dispatched:u64, input_end:u64 }
#[derive(Default)]
struct State { inputs:HashMap<u32,Input>, active:Option<u32>, sinks:HashMap<usize,bool>, raw_first:u64, raw_last:u64, converted_first:u64, converted_last:u64, pull_first:u64, raw_matching:u64, converted_matching:u64, frames:HashMap<usize,Value> }
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
thread_local! { static WINDOW:Cell<Option<u32>>=const { Cell::new(None) }; static INTERVAL:Cell<u64>=const { Cell::new(0) }; static SCAN_START:Cell<u64>=const {Cell::new(0)}; static SCAN_END:Cell<u64>=const {Cell::new(0)}; static STATUS_START:Cell<u64>=const {Cell::new(0)}; static STATUS_END:Cell<u64>=const {Cell::new(0)}; }
pub fn is_input(r:&Request)->bool { window(r).is_some() }
fn window(r:&Request)->Option<u32> {
    match r {
        Request::Pointer{window_id,button:1|3,pressed:true,..} |
        Request::Key{window_id,keysym:0xff51,pressed:true} => Some(*window_id),
        _=>None
    }
}
pub fn request(stage:&str,r:&Request) {
    if let Request::VideoFeedback{frame_id}=r {
        crate::selection_probe::trace(json!({"event":"video_feedback","stage":stage,"frame_id":frame_id,"timestamp_ns":now()}));
        return;
    }
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
pub fn status(start:bool){if start{STATUS_START.set(now());}else{STATUS_END.set(now());}}
pub fn publish(pointer:usize) {
    let mut s=STATE.get_or_init(Default::default).lock().unwrap();
    if let Some(v)=s.frames.get_mut(&pointer){v["publish"]=json!(now());v["scan_start"]=json!(SCAN_START.get());v["scan_end"]=json!(SCAN_END.get());}
    drop(s);crate::selection_probe::publish(pointer);
}
pub fn capture(window:u32,pointer:usize,start:u64) {
    let mut s=STATE.get_or_init(Default::default).lock().unwrap();
    let Some(i)=s.inputs.get(&window).cloned() else{return};
    static NEXT_CAPTURE: AtomicU64 = AtomicU64::new(1);
    let v=json!({"capture_id":NEXT_CAPTURE.fetch_add(1,Ordering::Relaxed),"count":i.count,"received":i.received,"dispatched":i.dispatched,
        "input_end":i.input_end,"capture_start":start,"capture_end":now(),"status_start":STATUS_START.get(),"status_end":STATUS_END.get(),"raw_first":s.raw_first,"raw_last":s.raw_last,"converted_first":s.converted_first,"converted_last":s.converted_last,"pull_first":s.pull_first,"raw_matching":s.raw_matching,"converted_matching":s.converted_matching,"interval":INTERVAL.get()});
    if s.frames.len()>128 {s.frames.clear();}
    s.frames.insert(pointer,v);
}
pub fn frame(pointer:usize,start:u64)->Value {
    let mut v=STATE.get_or_init(Default::default).lock().unwrap().frames.get(&pointer).cloned().unwrap_or(Value::Null);
    if v.is_object() {v["encode_start"]=json!(start);v["encode_end"]=json!(now());v["selection"]=crate::selection_probe::frame(pointer);}
    v
}
pub fn x11_source(pointer:usize, source:usize) {
    if let Some(frame)=STATE.get_or_init(Default::default).lock().unwrap().frames.get_mut(&pointer) {
        frame["source_image_data"]=json!(source);
    }
}
'''

def instrument(destination, revision, isolated_state=False, stages=True, source_overrides=(), trace_only=False, x11_capture_stages=False):
    if trace_only and not stages:
        raise ValueError('trace-only requires stage instrumentation')
    destination.mkdir(parents=True, exist_ok=False)
    with tempfile.TemporaryFile() as archive:
        subprocess.run(['git', 'archive', revision], cwd=ROOT, stdout=archive, check=True)
        archive.seek(0)
        subprocess.run(['tar', '-xf', '-', '-C', str(destination)], stdin=archive, check=True)
    source = destination / 'apps/linux-agent/src'
    for name in source_overrides:
        relative = Path(name)
        original = (ROOT / relative).resolve()
        target = (destination / relative).resolve()
        if (relative.is_absolute() or '..' in relative.parts
                or not original.is_relative_to(ROOT.resolve()) or not original.is_file()
                or not target.is_relative_to(destination.resolve()) or not target.is_file()):
            raise ValueError('source override must be a repository-relative file')
        target.write_bytes(original.read_bytes())
    def edit(name, before, after):
        p = source / name
        text = p.read_text()
        if text.count(before) != 1:
            raise ValueError(f'instrumentation anchor changed in {name}: {before[:80]}')
        p.write_text(text.replace(before, after))

    def edit_many(name, before, after, expected):
        p = source / name
        text = p.read_text()
        if text.count(before) != expected:
            raise ValueError(f'instrumentation anchor count changed in {name}: {before[:80]}')
        p.write_text(text.replace(before, after))
    if isolated_state:
        # Test-only builds never bind the installed Agent's socket or change
        # HOME. Its existing desktop permission can be restored independently.
        edit('main.rs', 'let p = home()?.join(".local/state/lan-remoteapp");',
             'let p = PathBuf::from(std::env::var("PANEFERRY_PROBE_STATE").context("probe state required")?);\n    if !p.is_absolute() {bail!("absolute probe state required")}')
        edit('desktop_input.py', "apps = Path(os.environ.get('XDG_DATA_HOME', str(Path.home() / '.local/share'))) / 'applications'",
             "apps = self.store.path.parent / 'probe-applications'")
    if not stages:
        print(destination)
        return
    if x11_capture_stages:
        backend = destination / 'crates/backend-x11/src'
        (backend / 'capture_probe.rs').write_bytes((ROOT / 'tests/performance/x11_capture_probe.rs').read_bytes())
        lib = backend / 'lib.rs'
        text = lib.read_text()
        if (backend / 'readback.rs').exists():
            text = text.replace('mod damage;', 'mod damage;\nmod capture_probe;')
            lib.write_text(text)
            readback = backend / 'readback.rs'
            text = readback.read_text()
            replacements = [
                ('    let pixmap = if surface.composite {', '    let mut probe = crate::capture_probe::CaptureProbe::new(surface.window);\n    let pixmap = if surface.composite {'),
                ('    let mut size = surface.source;', '    probe.mark("named_pixmap");\n    let mut size = surface.source;'),
                ('        if let Some(image) = shared.capture(', '        probe.mark("scaled");\n        if let Some(image) = shared.capture('),
                ('            return Ok(image);', '            probe.mark("readback");\n            return Ok(image);'),
                ('        Ok((image.depth, image.visual, image.data))', '        probe.mark("readback_fallback");\n        Ok((image.depth, image.visual, image.data))'),
                ('    Ok(CapturedImage {', '    let image = CapturedImage {'),
                ('    })\n}', '    };\n    probe.image(&image, surface.target);\n    probe.mark("metadata");\n    Ok(image)\n}'),
            ]
            worker = backend / 'async_capture.rs'
            work = worker.read_text()
            anchor = '        capture\n            .worker\n            .submit'
            if work.count(anchor) != 1:
                raise ValueError('asynchronous capture admission anchor changed')
            worker.write_text(work.replace(anchor, '        crate::capture_probe::queued(id);\n' + anchor))
            for before, after in replacements:
                if text.count(before) != 1:
                    raise ValueError('X11 readback timing anchor changed: ' + before[:80])
                text = text.replace(before, after)
            readback.write_text(text)
        else:
            replacements = [
            ('mod damage;', 'mod damage;\nmod capture_probe;'),
            ('fn capture_image(&self, w: &Window, wanted: (u32, u32)) -> Result<CapturedImage> {',
             'fn capture_image(&self, w: &Window, wanted: (u32, u32)) -> Result<CapturedImage> {\n        let mut probe = capture_probe::CaptureProbe::new(w.window_id);'),
            ('        self.known(w.window_id)?;\n        self.damage', '        self.known(w.window_id)?;\n        probe.mark("known");\n        self.damage'),
            ('            .acknowledge(&self.conn, w.window_id)?;', '            .acknowledge(&self.conn, w.window_id)?;\n        probe.mark("acknowledged");'),
            ('        let mut size = (w.width, w.height);', '        probe.mark("named_pixmap");\n        let mut size = (w.width, w.height);'),
            ('            if let Some(image) = self\n                .shared_capture', '            probe.mark("scaled");\n            if let Some(image) = self\n                .shared_capture'),
            ('                return Ok(image);\n            }\n            let image = self', '                probe.mark("readback");\n                return Ok(image);\n            }\n            let image = self'),
            ('            Ok((image.depth, image.visual, image.data))', '            probe.mark("readback_fallback");\n            Ok((image.depth, image.visual, image.data))'),
            ('        Ok(CapturedImage {', '        probe.mark("metadata");\n        Ok(CapturedImage {'),
            ]
            for before, after in replacements:
                if text.count(before) != 1:
                    raise ValueError('X11 capture timing anchor changed: ' + before[:80])
                text = text.replace(before, after)
            # Give the old synchronous path the same snapshot identity/marker.
            start=text.index('    fn capture_image(')
            end=text.index('    fn known(', start)
            block=text[start:end]
            block=block.replace('Ok(CapturedImage {', 'let image = CapturedImage {')
            at=block.rindex('})')
            block=block[:at]+'};\n        probe.image(&image, wanted);\n        Ok(image)'+block[at+2:]
            lib.write_text(text[:start]+block+text[end:])
    edit('wayland_rpc.rs', 'Call(Value, SyncSender<Reply>),', 'Call(Value, SyncSender<Reply>, u64),')
    edit_many('wayland_rpc.rs', 'Work::Call(value, reply) => {', 'Work::Call(value, reply, probe_id) => {', 2)
    edit_many('wayland_rpc.rs', 'let _ = reply.send(invoke(&mut call, value));',
              'crate::stage_probe::rpc_event(probe_id,"rpc_started");\n let result=invoke(&mut call,value);\n crate::stage_probe::rpc_event(probe_id,"rpc_completed");\n let _ = reply.send(result);', 2)
    edit('wayland_rpc.rs', 'pub fn call(&mut self, value: Value) -> Result<Value> {',
         'pub fn call(&mut self, value: Value) -> Result<Value> {\n        let probe_id=crate::stage_probe::rpc_queued("foreground",value["op"].as_str().unwrap_or("unknown"));')
    edit('wayland_rpc.rs', '.send(Work::Call(value, sender))?', '.send(Work::Call(value, sender, probe_id))?')
    edit('wayland_rpc.rs', 'snapshot[op] = invoke(&mut call, json!({"op":op}))?;',
         'let probe_id=crate::stage_probe::rpc_queued("metadata",op);\n crate::stage_probe::rpc_event(probe_id,"rpc_started");\n let result=invoke(&mut call,json!({"op":op}));\n crate::stage_probe::rpc_event(probe_id,"rpc_completed");\n snapshot[op]=result?;')
    for op in ('resize', 'scene_point'):
        edit('wayland_rpc.rs', f'Work::Call(json!({{"op":"{op}"}}), reply)', f'Work::Call(json!({{"op":"{op}"}}), reply, 0)')
    edit('desktop_input.rs', 'let mut slot = self.inner.broker.lock().unwrap();',
         'let probe_id=crate::stage_probe::rpc_queued("desktop",request["type"].as_str().unwrap_or("unknown"));\n        let mut slot = self.inner.broker.lock().unwrap();\n        crate::stage_probe::rpc_event(probe_id,"rpc_started");')
    edit('desktop_input.rs', 'let result = broker.call(request);',
         'let result = broker.call(request);\n        crate::stage_probe::rpc_event(probe_id,"rpc_completed");')
    edit('wayland.rs', 'fn call(&mut self, value: Value) -> Result<Value> {',
         'fn call(&mut self, value: Value) -> Result<Value> {\n let probe_id=crate::stage_probe::rpc_queued("bridge",value["op"].as_str().unwrap_or("unknown"));\n crate::stage_probe::rpc_event(probe_id,"rpc_started");')
    edit('wayland.rs', 'let answer: Value = serde_json::from_slice(&line)?;',
         'let answer: Value = serde_json::from_slice(&line)?;\n crate::stage_probe::rpc_event(probe_id,"rpc_completed");')
    edit('backend/logical_x11.rs', 'fn capture(&self, window: &Window) -> Result<Arc<crate::video::Frame>> {',
         'fn capture(&self, window: &Window) -> Result<Arc<crate::video::Frame>> {\n        let probe_capture=crate::stage_probe::now();')
    constructor = 'scaled_shared_image' if 'scaled_shared_image' in (source/'backend/logical_x11.rs').read_text() else 'scaled_image'
    edit('backend/logical_x11.rs', f'crate::video::Frame::{constructor}(image, window.width, window.height).map(Arc::new)',
         f'let probe_source=image.data.as_ptr() as usize;\n let frame=Arc::new(crate::video::Frame::{constructor}(image, window.width, window.height)?);\n crate::stage_probe::capture(window.window_id,Arc::as_ptr(&frame) as usize,probe_capture);\n crate::stage_probe::x11_source(Arc::as_ptr(&frame) as usize,probe_source);\n Ok(frame)')
    (source / 'stage_probe.rs').write_text(MODULE)
    for name in ['selection_probe.rs', 'selection_gstreamer.rs']:
        (source / name).write_text((ROOT / 'tests/performance' / name).read_text())
    edit('main.rs', 'static STOP:', 'mod stage_probe;\nmod selection_probe;\nstatic STOP:')
    edit('video.rs', 'type Object =', '#[path="selection_gstreamer.rs"]\nmod selection_gstreamer;\ntype Object =')
    edit('video.rs', 'caps_structure: unsafe extern', '''probe_caps: unsafe extern "C" fn(*const c_char) -> Object => (core, c"gst_caps_from_string"),
    probe_add_reference: unsafe extern "C" fn(Object, Object, u64, u64) -> Object => (core, c"gst_buffer_add_reference_timestamp_meta"),
    probe_get_reference: unsafe extern "C" fn(Object, Object) -> Object => (core, c"gst_buffer_get_reference_timestamp_meta"),
    caps_structure: unsafe extern''')
    edit('video.rs', 'pipewiresrc path={node} do-timestamp=true ! video/x-raw,format=BGRx ! appsink', 'pipewiresrc path={node} do-timestamp=true ! video/x-raw,format=BGRx ! identity name=probe_ready signal-handoffs=true ! appsink')
    edit('video_frame.rs', '! appsink name=sink sync=false max-buffers=1 drop=true', '! identity name=probe_ready signal-handoffs=true ! appsink name=sink sync=false max-buffers=1 drop=true')
    edit('main.rs', 'Ok(req) => {', 'Ok(req) => {\n                            stage_probe::request("received", &req);')
    edit('main.rs', 'if triggers_fast_scan(&req) {', 'let probe_input = stage_probe::is_input(&req);\n            stage_probe::request("dispatched", &req);\n            if triggers_fast_scan(&req) {')
    edit('main.rs', 'if let Err(e) = result {', 'if probe_input {stage_probe::input_end();}\n            if let Err(e) = result {')
    edit('main.rs', 'let status = b.input_status();', 'stage_probe::status(true);\n                let status = b.input_status();\n                stage_probe::status(false);')
    edit('video.rs', 'let signal = connect_wake(api, sink, wake)?;', 'crate::stage_probe::sink(sink as usize,true);\n        selection_gstreamer::connect(api,pipeline,sink,true,0,0)?;\n        let signal = connect_wake(api, sink, wake)?;')
    edit('video_frame.rs', 'self.signal = connect_wake(self.api, self.sink, wake)?;', 'crate::stage_probe::sink(self.sink as usize,false);\n        selection_gstreamer::connect(self.api,self.pipeline,self.sink,false,self.spec.region[2],self.spec.region[3])?;\n        self.signal = connect_wake(self.api, self.sink, wake)?;')
    edit('video.rs', 'let wake = unsafe { &*data.cast::<Arc<FrameWake>>() };', 'crate::stage_probe::wake(_sink as usize);\n    crate::selection_probe::ready(_sink as usize);\n    let wake = unsafe { &*data.cast::<Arc<FrameWake>>() };')
    edit('video_frame.rs', 'if unsafe { (self.api.push)(self.source, input) } != 0 {', 'selection_gstreamer::tag_input(self.api,buffer,input);\n        if unsafe { (self.api.push)(self.source, input) } != 0 {')
    edit('video.rs', 'let sample = unsafe { (self.api.pull)(self.sink, 0) };', 'crate::stage_probe::pulling();\n        let sample = unsafe { (self.api.pull)(self.sink, 0) };')
    edit('video.rs', 'converter.submit(buffer)?;', '''if spec.region[2]>=256 && spec.region[3]>16 {
                let mapping=ReadMap::new(self.api,buffer)?;
                let bytes=mapping.bytes()?;
                let mut marker=0u64;
                for bit in 0..8 {let offset=((spec.region[1]+16) as usize*spec.source_width as usize+spec.region[0] as usize+16+bit*32)*4;if bytes.get(offset).is_some_and(|v|*v>128){marker|=1<<bit;}}
                crate::stage_probe::pixels(true,marker);
                crate::selection_probe::pulled(buffer as usize,marker,true);
            }
            converter.submit(buffer)?;''')
    edit('video_frame.rs', 'Ok(Some(Frame {', '''if width>=256 && height>16 {
            let mapping=ReadMap::new(self.api,output)?;
            let bytes=mapping.bytes()?;
            let mut marker=0u64;
            for bit in 0..8 {let offset=16*w.next_multiple_of(4)+16+bit*32;if bytes.get(offset).is_some_and(|v|*v>128){marker|=1<<bit;}}
            crate::stage_probe::pixels(false,marker);
            crate::selection_probe::pulled(output as usize,marker,false);
        }
        crate::selection_probe::selected(output as usize);
        Ok(Some(Frame {''')
    edit('main.rs', 'let capture_due = capture_pending', 'stage_probe::interval(frame_interval.as_nanos() as u64);\n        selection_probe::deadline(last_capture.elapsed().as_nanos() as u64,frame_interval.as_nanos() as u64);\n        let capture_due = capture_pending')
    edit('main.rs', 'let mut windows = if !s.has_exited()? && scan_due {\n                match s.scan_windows() {', 'let mut windows = if !s.has_exited()? && scan_due {\n                stage_probe::scan(true);\n                let probe_scan_result=s.scan_windows();\n                stage_probe::scan(false);\n                match probe_scan_result {')
    edit('main.rs', 'out.frame(w, image, &sid)?;', 'stage_probe::publish(Arc::as_ptr(&image) as usize);\n                        out.frame(w, image, &sid)?;')
    edit('wayland.rs', 'let result = if self.scene.is_some() {', 'let probe_capture = crate::stage_probe::now();\n            let result = if self.scene.is_some() {')
    edit('wayland.rs', 'stream.image = Some(Arc::new(image));', 'let image = Arc::new(image);\n                crate::stage_probe::capture(*id, Arc::as_ptr(&image) as usize, probe_capture);\n                crate::selection_probe::capture(Arc::as_ptr(&image) as usize);\n                stream.image = Some(image);')
    edit('output_worker.rs', '            return Ok(None);\n        }\n        // At most 8 MP',
         '            crate::selection_probe::trace(json!({"event":"encoder_duplicate","window_id":id,"timestamp_ns":crate::stage_probe::now(),"stages":crate::stage_probe::frame(Arc::as_ptr(&job.image) as usize,crate::stage_probe::now())}));\n            return Ok(None);\n        }\n        // At most 8 MP')
    edit('output_worker.rs', 'let packet = state.encoder.encode_frame(&job.image)?;', 'let probe_encode = crate::stage_probe::now();\n        crate::selection_probe::encoding(Arc::as_ptr(&job.image) as usize,probe_encode);\n        let packet = state.encoder.encode_frame(&job.image)?;')
    edit('output_worker.rs', '"stream_id":state.encoder.stream,', '"_agent_stages":crate::stage_probe::frame(Arc::as_ptr(&job.image) as usize,probe_encode),\n            "stream_id":state.encoder.stream,')
    qos = source / 'video_qos.rs'
    q = qos.read_text()
    anchor = '        let excess = delay.saturating_sub(baseline);'
    if q.count(anchor) != 1:
        raise ValueError('QoS timing anchor changed')
    qos.write_text(q.replace(anchor, anchor + '\n        crate::selection_probe::trace(serde_json::json!({"event":"qos_feedback","frame_id":id,"timestamp_ns":crate::stage_probe::now(),"target":self.target,"cap":self.cap,"pending":self.pending.len(),"delay_ns":delay.as_nanos(),"baseline_ns":baseline.as_nanos(),"excess_ns":excess.as_nanos()}));'))
    edit('output.rs', 'let mut bytes = serde_json::to_vec(&value)?;', 'if value["_agent_stages"].is_object() {value["_agent_stages"]["write"]=json!(crate::stage_probe::now());}\n        let mut bytes = serde_json::to_vec(&value)?;')
    edit('output.rs', 'socket.write_all(&bytes)?;', 'let probe_write=crate::stage_probe::now();\n        socket.write_all(&bytes)?;\n        crate::selection_probe::sent(&value,bytes.len(),probe_write,crate::stage_probe::now());')
    if trace_only:
        edit('output.rs', 'let mut bytes = serde_json::to_vec(&value)?;',
             'let probe_stages = value.as_object_mut().and_then(|v| v.remove("_agent_stages"));\n        let mut bytes = serde_json::to_vec(&value)?;')
        edit('output.rs', 'crate::selection_probe::sent(&value,bytes.len(),probe_write,crate::stage_probe::now());',
             'let probe_done=crate::stage_probe::now();\n        if let Some(stages)=probe_stages {value["_agent_stages"]=stages;}\n        crate::selection_probe::sent(&value,bytes.len(),probe_write,probe_done);')
    print(destination)

if __name__ == '__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('destination', type=Path)
    p.add_argument('--revision', default='HEAD')
    p.add_argument('--isolated-state', action='store_true', help='require a private PANEFERRY_PROBE_STATE; keep HOME and installed desktop integration untouched')
    p.add_argument('--no-stages', action='store_true', help='archive only (plus optional isolated state), without timing instrumentation')
    p.add_argument('--trace-only', action='store_true', help='keep frame metadata unchanged; write timings only to PANEFERRY_PROBE_TRACE')
    p.add_argument('--x11-capture-stages', action='store_true', help='diagnostic-only wall and thread CPU checkpoints inside X11 readback')
    p.add_argument('--source-override', action='append', default=[], help='copy this explicit working-tree file before instrumenting the archive')
    a=p.parse_args()
    if a.trace_only and a.no_stages:
        p.error('--trace-only and --no-stages are mutually exclusive')
    if a.x11_capture_stages and a.no_stages:
        p.error('--x11-capture-stages requires stage instrumentation')
    instrument(a.destination.resolve(), a.revision, a.isolated_state, not a.no_stages, a.source_override, a.trace_only, a.x11_capture_stages)
