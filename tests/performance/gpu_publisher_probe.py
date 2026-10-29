#!/usr/bin/env python3
"""Extend an isolated publisher probe with readback CPU/wall and codec pad timings.

No production source, installed Agent, pixels, input or wire fields are changed.
Use only with a test-owned window and a fresh local trace path.
"""
import argparse
from pathlib import Path
import re
import subprocess
import sys


DETAILS = r'''
thread_local! {
    static DETAILS:std::cell::RefCell<Vec<Value>>=const {std::cell::RefCell::new(Vec::new())};
}
pub fn detail(stage:&str) {
    let mut cpu=libc::timespec{tv_sec:0,tv_nsec:0};
    assert_eq!(unsafe{libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID,&mut cpu)},0);
    DETAILS.with_borrow_mut(|points|points.push(json!({"stage":stage,"timestamp_ns":now(),
        "thread_cpu_ns":cpu.tv_sec as u64*1_000_000_000+cpu.tv_nsec as u64,
        "thread_id":unsafe{libc::syscall(libc::SYS_gettid)}})));
}
pub fn begin() {DETAILS.with_borrow_mut(Vec::clear);detail("capture.begin");}
fn details()->Vec<Value> {DETAILS.with_borrow_mut(std::mem::take)}
pub fn detail_value(value:Value) {DETAILS.with_borrow_mut(|v|v.push(value));}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('destination', type=Path)
    parser.add_argument('--revision', default='HEAD')
    parser.add_argument('--source-override', action='append', default=[])
    parser.add_argument('--encoder-source', type=Path,
                        help='optional experimental video.rs; never installed automatically')
    args = parser.parse_args()
    command = [sys.executable, str(Path(__file__).with_name('publisher_stage_probe.py')),
               str(args.destination), '--revision', args.revision]
    for name in args.source_override:
        command.extend(['--source-override', name])
    subprocess.run(command, check=True)
    destination = args.destination.resolve()
    backend = destination / 'crates/backend-x11/src'
    agent = destination / 'apps/linux-agent/src'
    if 'fn capture_video(' in (backend/'readback.rs').read_text():
        raise ValueError('GPU detail instrumentation targets snapshot capture: use --revision bbc10c4 for historical replay; use publisher_stage_probe.py for current DMA-buf leases')
    if args.encoder_source:
        (agent / 'video.rs').write_bytes(args.encoder_source.read_bytes())

    def edit(path, before, after):
        text = path.read_text()
        if text.count(before) != 1:
            raise ValueError(f'GPU detail anchor changed in {path.name}: {before[:70]}')
        path.write_text(text.replace(before, after))

    timing = backend / 'publisher_timing.rs'
    timing.write_text(timing.read_text() + DETAILS)
    (backend / 'gpu_fence_probe.rs').write_bytes(Path(__file__).with_name('gpu_fence_probe.rs').read_bytes())
    edit(backend / 'lib.rs', 'pub mod publisher_timing;', 'pub mod publisher_timing;\nmod gpu_fence_probe;')
    edit(timing, '"capture_backend":backend}',
         '"capture_backend":backend,"readback_detail":details()}')
    readback = backend / 'readback.rs'
    edit(readback, '    let probe_start=crate::publisher_timing::now();',
         '    let probe_start=crate::publisher_timing::now();\n    crate::publisher_timing::begin();')
    for anchor, label in [
        ('    let mut size = surface.source;', 'source.named'),
        ('        if let Some(visual) = surface.visual {', 'source.geometry_checked'),
        ('        if let Some(mapped) = render.capture_linear', 'render.submitted'),
        ('    let (depth, visual_id, data) = image?;', 'snapshot.complete'),
    ]:
        edit(readback, anchor, f'    crate::publisher_timing::detail("{label}");\n' + anchor)
    for anchor, label in [
        ('            let reset = if let Some(fence) = self.fence {', 'linear.submit_begin'),
        ('            let end = Instant::now() + Duration::from_secs(1);', 'linear.export_received'),
        ('            sync(self.fd.as_raw_fd(), false)?;', 'linear.fence_ready'),
        ('            let row = self.size.0 as usize * 4;', 'linear.copy_begin'),
        ('            sync(self.fd.as_raw_fd(), true)?;', 'linear.copy_end'),
        ('            std::mem::forget(end);', 'linear.coherency_ended'),
    ]:
        edit(backend / 'linear_capture.rs', anchor,
             f'            crate::publisher_timing::detail("{label}");\n' + anchor)
    edit(backend / 'linear_capture.rs', '            let end = Instant::now() + Duration::from_secs(1);',
         '            let probe_fence=crate::gpu_fence_probe::export(self.fd.as_raw_fd());\n'
         '            let end = Instant::now() + Duration::from_secs(1);')
    edit(backend / 'linear_capture.rs', '            sync(self.fd.as_raw_fd(), false)?;',
         '            crate::gpu_fence_probe::observe(probe_fence.as_ref());\n'
         '            sync(self.fd.as_raw_fd(), false)?;')

    probe = Path(__file__).with_name('codec_probe.rs').read_text()
    probe = probe.replace('crate::stage_probe::now()', 'crate::publisher_timing::now()')
    probe = probe.replace('(c"encoder", "codec"),',
                          '(c"probe_interleave", "interleave"),\n'
                          '            (c"probe_upload", "upload"),\n'
                          '            (c"encoder", "codec"),')
    begin = probe.index('    pub fn finish(')
    end = probe.index('\n}\nimpl Drop for Probe', begin)
    probe = probe[:begin] + '''    pub fn finish(&self) -> Vec<Value> {
        self.mark("encode.end");
        std::mem::take(&mut *self.points.0.lock().unwrap())
    }''' + probe[end:]
    (agent / 'gpu_codec_probe.rs').write_text(probe)
    video = agent / 'video.rs'
    edit(video, 'type Object =', '#[path="gpu_codec_probe.rs"]\nmod gpu_codec_probe;\ntype Object =')
    edit(video, '    caps_structure: unsafe extern', '''    probe_pad: unsafe extern "C" fn(Object, *const c_char) -> Object => (core, c"gst_element_get_static_pad"),
    probe_install: unsafe extern "C" fn(Object, c_uint, unsafe extern "C" fn(Object,Object,Object)->c_int, Object, Option<Destroy>) -> libc::c_ulong => (core, c"gst_pad_add_probe"),
    probe_remove: unsafe extern "C" fn(Object, libc::c_ulong) -> () => (core, c"gst_pad_remove_probe"),
    caps_structure: unsafe extern''')
    edit(video, '    first_frame_fallback: Option<u8>,',
         '    first_frame_fallback: Option<u8>,\n    gpu_probe: Option<gpu_codec_probe::Probe>,')
    edit(video, '            first_frame_fallback: None,',
         '            first_frame_fallback: None,\n            gpu_probe: None,')
    edit(video, '        Ok(encoder)\n    }',
         '        encoder.gpu_probe=Some(gpu_codec_probe::Probe::new(api,pipeline)?);\n        Ok(encoder)\n    }')
    edit(video, '    pub keyframe: bool,',
         '    pub keyframe: bool,\n    pub gpu_detail:Vec<serde_json::Value>,')
    edit(video, '        let buffer = image.input_buffer(self.api)?;',
         '        self.gpu_probe.as_ref().unwrap().begin();\n'
         '        let buffer = image.input_buffer(self.api)?;\n'
         '        self.gpu_probe.as_ref().unwrap().mark("input.wrapped");')
    edit(video, '        let sample = unsafe { (self.api.pull)(self.sink, 2_000_000_000) };',
         '        self.gpu_probe.as_ref().unwrap().mark("input.pushed");\n'
         '        let sample = unsafe { (self.api.pull)(self.sink, 2_000_000_000) };\n'
         '        self.gpu_probe.as_ref().unwrap().mark("output.pulled");')
    edit(video, '        Ok(Packet { bytes, keyframe })',
         '        Ok(Packet {bytes,keyframe,gpu_detail:self.gpu_probe.as_ref().unwrap().finish()})')
    edit(video, '            if !self.source.is_null() {\n                (self.api.unref)(self.source);',
         '            self.gpu_probe.take();\n'
         '            if !self.source.is_null() {\n                (self.api.unref)(self.source);')
    text = video.read_text()
    begin = text.index('fn pipeline_for_format(')
    end = text.index('\nimpl Encoder', begin)
    def label(match):
        name = match[0]
        label = {'videobox': 'padding', 'videoscale': 'scaling', 'vapostproc': 'upload'}.get(name)
        if not label:
            label = 'conversion' if text[begin:end][match.end():].startswith(' n-threads=') else 'interleave'
        return name + ' name=probe_' + label
    block = re.sub(r'\b(videobox|videoscale|videoconvert|vapostproc)(?=\s)', label, text[begin:end])
    video.write_text(text[:begin] + block + text[end:])
    worker = agent / 'output_worker.rs'
    edit(worker, 'probe["encode_end_ns"]=json!(crate::publisher_timing::now());',
         'probe["encode_end_ns"]=json!(crate::publisher_timing::now());'
         'probe["codec_detail"]=json!(packet.gpu_detail);')


if __name__ == '__main__':
    main()
