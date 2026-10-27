//! Opt-in diagnostic archive only: pad boundaries without adding elements or
//! touching buffers. One encode is in flight per Encoder, so its pad callbacks
//! share an unambiguous frame identity even on a GStreamer streaming thread.
use super::*;
use serde_json::{Value, json};

#[derive(Default)]
struct Points(Mutex<Vec<Value>>);
struct Context {
    points: Arc<Points>,
    label: String,
}
fn stamp(label: &str) -> Value {
    let mut cpu = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut cpu) };
    json!({"stage":label,"timestamp_ns":crate::stage_probe::now(),
        "thread_cpu_ns":cpu.tv_sec as u64*1_000_000_000+cpu.tv_nsec as u64,
        "thread_id":unsafe{libc::syscall(libc::SYS_gettid)}})
}
unsafe extern "C" fn callback(_pad: Object, _info: Object, data: Object) -> c_int {
    let c = unsafe { &*data.cast::<Context>() };
    c.points.0.lock().unwrap().push(stamp(&c.label));
    1 // GST_PAD_PROBE_OK; pass the original buffer without mapping it.
}
unsafe extern "C" fn destroy(data: Object) {
    drop(unsafe { Box::from_raw(data.cast::<Context>()) });
}
pub(super) struct Probe {
    api: &'static Api,
    points: Arc<Points>,
    pads: Vec<(Object, libc::c_ulong)>,
}
impl Probe {
    pub fn new(api: &'static Api, pipeline: Object) -> Result<Self> {
        let mut probe = Self {
            api,
            points: Arc::default(),
            pads: Vec::new(),
        };
        for (name, label) in [
            (c"source", "appsrc"),
            (c"probe_padding", "padding"),
            (c"probe_scaling", "scaling"),
            (c"probe_conversion", "conversion"),
            (c"encoder", "codec"),
            (c"sink", "appsink"),
        ] {
            let element = unsafe { (api.get)(pipeline, name.as_ptr()) };
            if element.is_null() {
                continue;
            }
            for (name, suffix) in [(c"sink", "sink"), (c"src", "src")] {
                let pad = unsafe { (api.probe_pad)(element, name.as_ptr()) };
                if pad.is_null() {
                    continue;
                }
                let context = Box::into_raw(Box::new(Context {
                    points: probe.points.clone(),
                    label: format!("{label}.{suffix}"),
                }))
                .cast();
                let id =
                    unsafe { (api.probe_install)(pad, 1 << 4, callback, context, Some(destroy)) };
                if id == 0 {
                    unsafe {
                        destroy(context);
                        (api.unref)(pad);
                        (api.unref)(element);
                    }
                    bail!("cannot install codec diagnostic probe");
                }
                probe.pads.push((pad, id));
            }
            unsafe { (api.unref)(element) };
        }
        Ok(probe)
    }
    pub fn begin(&self) {
        let mut p = self.points.0.lock().unwrap();
        p.clear();
        p.push(stamp("encode.begin"));
    }
    pub fn mark(&self, label: &str) {
        self.points.0.lock().unwrap().push(stamp(label));
    }
    pub fn finish(&self, image: &Frame) {
        self.mark("encode.end");
        crate::selection_probe::trace(json!({"event":"codec_trace",
            "image_data":image as *const Frame as usize,
            "width":image.width,"height":image.height,
            "points":*self.points.0.lock().unwrap()}));
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        for (pad, id) in self.pads.drain(..) {
            unsafe {
                (self.api.probe_remove)(pad, id);
                (self.api.unref)(pad);
            }
        }
    }
}
