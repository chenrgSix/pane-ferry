//! Diagnostic-only buffer identity hooks. No additional pixel allocation/queue.
use super::*;
struct Context {
    sink: usize,
    raw: bool,
    width: u32,
    height: u32,
}
#[repr(C)]
struct ReferenceTimestamp {
    flags: c_uint,
    info: *const c_void,
    reference: Object,
    timestamp: u64,
    duration: u64,
}
fn reference(api: &'static Api) -> Object {
    static CAPS: OnceLock<usize> = OnceLock::new();
    *CAPS.get_or_init(|| unsafe {
        (api.probe_caps)(c"timestamp/x-paneferry-probe-monotonic".as_ptr()) as usize
    }) as Object
}
unsafe extern "C" fn destroy(data: Object, _closure: Object) {
    drop(unsafe { Box::from_raw(data.cast::<Context>()) });
}
unsafe extern "C" fn handoff(_identity: Object, buffer: Object, data: Object) {
    let c = unsafe { &*data.cast::<Context>() };
    let mut marker = None;
    let mut raw_ns = 0;
    if !c.raw {
        let api = api().unwrap();
        let meta = unsafe { (api.probe_get_reference)(buffer, reference(api)) }
            .cast::<ReferenceTimestamp>();
        if !meta.is_null() {
            raw_ns = unsafe { (*meta).timestamp };
        }
        if c.width >= 256 && c.height > 16 {
            if let Ok(map) = ReadMap::new(api, buffer) {
                if let Ok(bytes) = map.bytes() {
                    let w = (c.width + c.width % 2) as usize;
                    let mut value = 0;
                    for bit in 0..8 {
                        if bytes
                            .get(16 * w.next_multiple_of(4) + 16 + bit * 32)
                            .is_some_and(|v| *v > 128)
                        {
                            value |= 1 << bit;
                        }
                    }
                    marker = Some(value);
                }
            }
        }
    }
    crate::selection_probe::produced(c.sink, buffer as usize, c.raw, marker, raw_ns);
}
pub(super) fn connect(
    api: &'static Api,
    pipeline: Object,
    sink: Object,
    raw: bool,
    width: u32,
    height: u32,
) -> Result<()> {
    let identity = unsafe { (api.get)(pipeline, c"probe_ready".as_ptr()) };
    if identity.is_null() {
        bail!("missing diagnostic identity")
    }
    crate::selection_probe::sink(sink as usize);
    let data = Box::into_raw(Box::new(Context {
        sink: sink as usize,
        raw,
        width,
        height,
    }))
    .cast();
    let cb = unsafe {
        std::mem::transmute::<unsafe extern "C" fn(Object, Object, Object), unsafe extern "C" fn()>(
            handoff,
        )
    };
    let id =
        unsafe { (api.signal_connect)(identity, c"handoff".as_ptr(), cb, data, Some(destroy), 0) };
    unsafe { (api.unref)(identity) };
    if id == 0 {
        unsafe { destroy(data, ptr::null_mut()) };
        bail!("cannot connect diagnostic identity")
    }
    Ok(())
}
pub(super) fn tag_input(api: &'static Api, raw: Object, input: Object) {
    if let Some(timestamp) = crate::selection_probe::raw_timestamp(raw as usize) {
        unsafe { (api.probe_add_reference)(input, reference(api), timestamp, u64::MAX) };
    }
}
