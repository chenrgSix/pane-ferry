//! Runtime-loaded GStreamer C API: share captured pixels, without a raw-frame subprocess.
use anyhow::{bail, Context, Result};
use paneferry_backend_x11::CapturedImage;
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    ffi::{c_char, c_int, c_uint, c_void, CStr, CString},
    io::{Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    ptr,
    sync::{Arc, Mutex, OnceLock},
};

#[path = "video_frame.rs"]
mod frame;
use frame::{CropConverter, CropSpec, Sample};
pub use frame::{Format, Frame};

type Object = *mut c_void;
type Destroy = unsafe extern "C" fn(Object);
type WindowCrop = ([u32; 4], (u32, u32), f64);
const MEMORY_READONLY: c_uint = 1 << 1;
const BUFFER_DELTA_UNIT: c_uint = 1 << 13;
const STATE_NULL: c_int = 1;
const STATE_PLAYING: c_int = 4;

// Concrete public C signatures and opaque pointers only: no GObject/GstBuffer
// layout assumptions. Libraries intentionally stay loaded for process-global
// plugin state and destroy callbacks running on GStreamer streaming threads.
macro_rules! runtime_api {
    ($core:ident, $app:ident, $glib:ident, $video:ident, $gobject:ident; $($field:ident: $signature:ty => ($library:ident, $symbol:expr)),+ $(,)?) => {
        struct Api { $($field: $signature),+ }
        impl Api {
            fn load() -> Result<Self> {
                unsafe fn library(name: &CStr) -> Result<Object> {
                    let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
                    if handle.is_null() { bail!("GStreamer runtime unavailable") }
                    Ok(handle)
                }
                unsafe {
                    let $core = library(c"libgstreamer-1.0.so.0")?;
                    let $app = library(c"libgstapp-1.0.so.0")?;
                    let $glib = library(c"libglib-2.0.so.0")?;
                    let $video = library(c"libgstvideo-1.0.so.0")?;
                    let $gobject = library(c"libgobject-2.0.so.0")?;
                    Ok(Self { $($field: {
                        let address = libc::dlsym($library, $symbol.as_ptr());
                        if address.is_null() { bail!("GStreamer API unavailable") }
                        std::mem::transmute::<Object, $signature>(address)
                    }),+ })
                }
            }
        }
    };
}
runtime_api! {
    core, app, glib, video, gobject;
    init: unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char, *mut Object) -> c_int => (core, c"gst_init_check"),
    factory: unsafe extern "C" fn(*const c_char) -> Object => (core, c"gst_element_factory_find"),
    parse: unsafe extern "C" fn(*const c_char, *mut Object) -> Object => (core, c"gst_parse_launch"),
    get: unsafe extern "C" fn(Object, *const c_char) -> Object => (core, c"gst_bin_get_by_name"),
    state: unsafe extern "C" fn(Object, c_int) -> c_int => (core, c"gst_element_set_state"),
    pipeline_clock: unsafe extern "C" fn(Object, Object) -> () => (core, c"gst_pipeline_use_clock"),
    unref: unsafe extern "C" fn(Object) -> () => (core, c"gst_object_unref"),
    free_error: unsafe extern "C" fn(Object) -> () => (glib, c"g_error_free"),
    copy_region: unsafe extern "C" fn(Object, c_uint, usize, usize) -> Object => (core, c"gst_buffer_copy_region"),
    parent_buffer: unsafe extern "C" fn(Object, Object) -> Object => (core, c"gst_buffer_add_parent_buffer_meta"),
    mini_ref: unsafe extern "C" fn(Object) -> Object => (core, c"gst_mini_object_ref"),
    mini_unref: unsafe extern "C" fn(Object) -> () => (core, c"gst_mini_object_unref"),
    wrap: unsafe extern "C" fn(c_uint, Object, usize, usize, usize, Object, Option<Destroy>) -> Object => (core, c"gst_buffer_new_wrapped_full"),
    push: unsafe extern "C" fn(Object, Object) -> c_int => (app, c"gst_app_src_push_buffer"),
    pull: unsafe extern "C" fn(Object, u64) -> Object => (app, c"gst_app_sink_try_pull_sample"),
    sample_buffer: unsafe extern "C" fn(Object) -> Object => (core, c"gst_sample_get_buffer"),
    sample_caps: unsafe extern "C" fn(Object) -> Object => (core, c"gst_sample_get_caps"),
    caps_structure: unsafe extern "C" fn(Object, c_uint) -> Object => (core, c"gst_caps_get_structure"),
    structure_int: unsafe extern "C" fn(Object, *const c_char, *mut c_int) -> c_int => (core, c"gst_structure_get_int"),
    sample_unref: unsafe extern "C" fn(Object) -> () => (core, c"gst_sample_unref"),
    size: unsafe extern "C" fn(Object) -> usize => (core, c"gst_buffer_get_size"),
    extract: unsafe extern "C" fn(Object, usize, Object, usize) -> usize => (core, c"gst_buffer_extract"),
    map: unsafe extern "C" fn(Object, *mut MapInfo, c_uint) -> c_int => (core, c"gst_buffer_map"),
    unmap: unsafe extern "C" fn(Object, *mut MapInfo) -> () => (core, c"gst_buffer_unmap"),
    crop_type: unsafe extern "C" fn() -> usize => (video, c"gst_video_crop_meta_api_get_type"),
    meta: unsafe extern "C" fn(Object, usize) -> *const VideoCropMeta => (core, c"gst_buffer_get_meta"),
    flags: unsafe extern "C" fn(Object) -> c_uint => (core, c"gst_buffer_get_flags"),
    signal_connect: unsafe extern "C" fn(Object, *const c_char, unsafe extern "C" fn(), Object, Option<unsafe extern "C" fn(Object, Object)>, c_uint) -> libc::c_ulong => (gobject, c"g_signal_connect_data"),
    signal_disconnect: unsafe extern "C" fn(Object, libc::c_ulong) -> () => (gobject, c"g_signal_handler_disconnect"),
}
impl Api {
    fn initialize(&self) -> Result<()> {
        let mut error = ptr::null_mut();
        // Null argc/argv is supported. Free every GError without forwarding
        // arbitrary plugin/runtime diagnostics into the wire protocol.
        let success = unsafe { (self.init)(ptr::null_mut(), ptr::null_mut(), &mut error) };
        if !error.is_null() {
            unsafe { (self.free_error)(error) };
        }
        if success == 0 {
            bail!("GStreamer initialization failed")
        }
        Ok(())
    }
    fn encoder_available(&self) -> bool {
        for name in [
            c"appsrc",
            c"identity",
            c"videoconvert",
            c"videobox",
            c"h264parse",
            c"appsink",
        ] {
            if !self.has_factory(name) {
                return false;
            }
        }
        self.has_factory(c"x264enc") || self.has_factory(c"openh264enc")
    }
    fn has_factory(&self, name: &CStr) -> bool {
        let factory = unsafe { (self.factory)(name.as_ptr()) };
        if factory.is_null() {
            return false;
        }
        unsafe { (self.unref)(factory) };
        true
    }
}
fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let api = Api::load().ok()?;
        api.initialize().ok()?;
        Some(api)
    })
    .as_ref()
}
pub fn available() -> bool {
    static ENCODER: OnceLock<bool> = OnceLock::new();
    *ENCODER.get_or_init(|| api().is_some_and(Api::encoder_available))
}

// Public GstMeta / GstVideoCropMeta C ABI from gstmeta.h / gstvideometa.h.
// GstBuffer and GstSample remain opaque; no private object layouts are read.
#[repr(C)]
struct VideoCropMeta {
    flags: c_uint,
    info: *const c_void,
    x: c_uint,
    y: c_uint,
    width: c_uint,
    height: c_uint,
}

// Public GstMapInfo ABI (gst/gstmemory.h), including GST_PADDING = 4.
#[repr(C)]
struct MapInfo {
    memory: Object,
    flags: c_uint,
    data: *mut u8,
    size: usize,
    maxsize: usize,
    user_data: [Object; 4],
    reserved: [Object; 4],
}
struct ReadMap {
    api: &'static Api,
    buffer: Object,
    info: MapInfo,
}
impl ReadMap {
    // The caller keeps the sample/buffer alive until this mapping is dropped.
    fn new(api: &'static Api, buffer: Object) -> Result<Self> {
        let mut mapping = Self {
            api,
            buffer,
            info: unsafe { std::mem::zeroed() },
        };
        if unsafe { (api.map)(buffer, &mut mapping.info, 1) } == 0 {
            // A failed map must not be unmapped.
            mapping.buffer = ptr::null_mut();
            bail!("cannot map PipeWire frame")
        }
        Ok(mapping)
    }
    fn bytes(&self) -> Result<&[u8]> {
        if self.info.data.is_null() || self.info.size > isize::MAX as usize {
            bail!("invalid PipeWire mapping")
        }
        Ok(unsafe { std::slice::from_raw_parts(self.info.data, self.info.size) })
    }
}
impl Drop for ReadMap {
    fn drop(&mut self) {
        if !self.buffer.is_null() {
            unsafe { (self.api.unmap)(self.buffer, &mut self.info) };
        }
    }
}

#[cfg(test)]
fn copy_bgrx_crop(bytes: &[u8], width: usize, crop: [usize; 4]) -> Result<Vec<u8>> {
    let [x, y, w, h] = crop;
    let source_stride = width.checked_mul(4).context("invalid PipeWire stride")?;
    let stride = w.checked_mul(4).context("invalid PipeWire crop")?;
    let end = y
        .checked_add(h)
        .and_then(|rows| rows.checked_mul(source_stride));
    if w == 0
        || h == 0
        || x.checked_add(w).is_none_or(|end| end > width)
        || end.is_none_or(|end| end > bytes.len())
    {
        bail!("invalid PipeWire crop")
    }
    let start = y * source_stride + x * 4;
    if w == width {
        return Ok(bytes[start..start + stride * h].to_vec());
    }
    // Allocate once without clearing a whole frame before overwriting it.
    let mut data = Vec::with_capacity(stride * h);
    for row in 0..h {
        let offset = start + row * source_stride;
        data.extend_from_slice(&bytes[offset..offset + stride]);
    }
    Ok(data)
}

/// A single compositor-selected window, retaining original pixels and crop metadata.
pub struct PipeWireCapture {
    api: &'static Api,
    pipeline: Object,
    sink: Object,
    signal: libc::c_ulong,
    converter: RefCell<Option<CropConverter>>,
    converter_window: Cell<Option<WindowCrop>>,
    pending_geometry: RefCell<Option<Sample>>,
    wake: Arc<FrameWake>,
}
/// All streams in one session share a single pollable wake. The appsink
/// callbacks only mark readiness; conversion runs on a GStreamer thread.
pub struct FrameWake {
    reader: UnixStream,
    writer: Mutex<(UnixStream, bool)>,
}
impl FrameWake {
    pub fn new() -> Result<Self> {
        let (reader, writer) = UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        writer.set_nonblocking(true)?;
        Ok(Self {
            reader,
            writer: Mutex::new((writer, false)),
        })
    }
    pub fn event_fd(&self) -> std::os::fd::RawFd {
        self.reader.as_raw_fd()
    }
    fn notify(&self) {
        let mut state = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        if !state.1 && state.0.write(&[1]).is_ok() {
            state.1 = true;
        }
    }
    pub fn drain(&self) -> bool {
        let mut state = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let pending = state.1;
        let mut bytes = [0; 64];
        while matches!((&self.reader).read(&mut bytes), Ok(n) if n > 0) {}
        state.1 = false;
        pending
    }
}
unsafe extern "C" fn sample_ready(_sink: Object, data: Object) -> c_int {
    let wake = unsafe { &*data.cast::<Arc<FrameWake>>() };
    wake.notify();
    0 // GST_FLOW_OK
}
unsafe extern "C" fn release_wake(data: Object, _closure: Object) {
    drop(unsafe { Box::from_raw(data.cast::<Arc<FrameWake>>()) });
}
fn connect_wake(api: &'static Api, sink: Object, wake: Arc<FrameWake>) -> Result<libc::c_ulong> {
    let data = Box::into_raw(Box::new(wake)).cast();
    let callback = unsafe {
        std::mem::transmute::<unsafe extern "C" fn(Object, Object) -> c_int, unsafe extern "C" fn()>(
            sample_ready,
        )
    };
    let signal = unsafe {
        (api.signal_connect)(
            sink,
            c"new-sample".as_ptr(),
            callback,
            data,
            Some(release_wake),
            0,
        )
    };
    if signal == 0 {
        unsafe { release_wake(data, ptr::null_mut()) };
        bail!("wayland_capture_unavailable")
    }
    Ok(signal)
}
// GStreamer objects are thread-safe. The session mutex serializes pull/drop;
// the streaming thread is stopped before the final references are released.
unsafe impl Send for PipeWireCapture {}
impl PipeWireCapture {
    pub fn new(node: u32, wake: Arc<FrameWake>) -> Result<Self> {
        let description = CString::new(format!("pipewiresrc path={node} do-timestamp=true ! video/x-raw,format=BGRx ! appsink name=frames max-buffers=1 drop=true sync=false emit-signals=true enable-last-sample=false"))?;
        Self::from_description(&description, wake)
    }
    fn from_description(description: &CStr, wake: Arc<FrameWake>) -> Result<Self> {
        let api = api().context("wayland_capture_unavailable")?;
        let mut error = ptr::null_mut();
        let pipeline = unsafe { (api.parse)(description.as_ptr(), &mut error) };
        if !error.is_null() {
            unsafe {
                (api.free_error)(error);
                if !pipeline.is_null() {
                    (api.unref)(pipeline);
                }
            }
            bail!("wayland_capture_unavailable")
        }
        if pipeline.is_null() {
            bail!("wayland_capture_unavailable")
        }
        // The compositor already paces PipeWire production. This capture-only
        // pipeline must deliver ready samples immediately, rather than also
        // waiting on GstBaseSrc's presentation clock before reaching appsink.
        // appsink sync=false alone only removes the sink's clock wait.
        // Encoding/publication is independently capped by the controller.
        unsafe { (api.pipeline_clock)(pipeline, ptr::null_mut()) };
        let sink = unsafe { (api.get)(pipeline, c"frames".as_ptr()) };
        let capture = Self {
            api,
            pipeline,
            sink,
            signal: 0,
            converter: RefCell::new(None),
            converter_window: Cell::new(None),
            pending_geometry: RefCell::new(None),
            wake: wake.clone(),
        };
        if sink.is_null() {
            bail!("wayland_capture_unavailable")
        }
        let signal = connect_wake(api, sink, wake)?;
        let mut capture = capture;
        capture.signal = signal;
        if unsafe { (api.state)(pipeline, STATE_PLAYING) } == 0 {
            bail!("wayland_capture_unavailable")
        }
        Ok(capture)
    }
    pub fn pull(
        &self,
        frame: [u32; 4],
        expected_buffer: (u32, u32),
        display_scale: f64,
    ) -> Result<Option<Frame>> {
        self.pull_inner(Some((frame, expected_buffer, display_scale)))
    }
    /// A monitor stream has no window crop. Preserve its actual captured pixels;
    /// the caller updates geometry before publishing a frame after a mode change.
    pub fn pull_monitor(&self) -> Result<Option<Frame>> {
        self.pull_inner(None)
    }
    pub fn has_pending_geometry(&self) -> bool {
        self.pending_geometry.borrow().is_some()
    }
    fn pull_inner(&self, window: Option<WindowCrop>) -> Result<Option<Frame>> {
        if self.converter_window.get() != window {
            self.converter.borrow_mut().take();
            self.converter_window.set(window);
        }
        let sample = unsafe { (self.api.pull)(self.sink, 0) };
        let sample = if sample.is_null() {
            match self.pending_geometry.borrow_mut().take() {
                Some(sample) => sample,
                None => {
                    return self
                        .converter
                        .borrow()
                        .as_ref()
                        .map_or(Ok(None), |c| c.pull(0))
                }
            }
        } else {
            // Keep only the latest producer sample. A frame may arrive before
            // its compositor geometry, especially when preparation is not
            // delayed until the next video publication deadline.
            self.pending_geometry.borrow_mut().take();
            Sample(sample, self.api)
        };
        let mut geometry_pending = false;
        let result = (|| {
            let caps = unsafe { (self.api.sample_caps)(sample.0) };
            if caps.is_null() {
                bail!("invalid PipeWire caps")
            }
            let structure = unsafe { (self.api.caps_structure)(caps, 0) };
            if structure.is_null() {
                bail!("invalid PipeWire caps")
            }
            let (mut width, mut height) = (0, 0);
            if unsafe { (self.api.structure_int)(structure, c"width".as_ptr(), &mut width) } == 0
                || unsafe { (self.api.structure_int)(structure, c"height".as_ptr(), &mut height) }
                    == 0
                || width <= 0
                || height <= 0
            {
                bail!("invalid PipeWire dimensions")
            }
            if width > 8192
                || height > 8192
                || u64::from(width as u32) * u64::from(height as u32) > 33_554_432
            {
                bail!("invalid PipeWire dimensions")
            }
            let buffer = unsafe { (self.api.sample_buffer)(sample.0) };
            if buffer.is_null() {
                bail!("missing PipeWire buffer")
            }
            let length = width as usize * height as usize * 4;
            if unsafe { (self.api.size)(buffer) } != length {
                bail!("invalid PipeWire stride")
            }
            let crop = unsafe { (self.api.meta)(buffer, (self.api.crop_type)()) };
            if crop.is_null() && window.is_some() {
                bail!("wayland_capture_unavailable")
            }
            let (crop_x, crop_y, mut crop_width, mut crop_height) = if crop.is_null() {
                (0, 0, width as u32, height as u32)
            } else {
                let crop = unsafe { &*crop };
                (crop.x, crop.y, crop.width, crop.height)
            };
            let [left, top, frame_width, frame_height] = match window {
                Some((frame, buffer, scale)) => {
                    let Some(frame) =
                        window_pixel_crop(frame, buffer, scale, (crop_width, crop_height))
                    else {
                        geometry_pending = true;
                        return Ok(None);
                    };
                    // Mutter's CPU renderer composites multi-surface windows
                    // at resource scale, while VideoCrop can still describe
                    // the root surface in logical units. The full rendered
                    // rectangle must fit in this frame's negotiated buffer.
                    if (crop_width, crop_height) == buffer && scale != 1.0 {
                        if crop_x != 0 || crop_y != 0 {
                            return Ok(None);
                        }
                        crop_width = (f64::from(buffer.0) * scale).round() as u32;
                        crop_height = (f64::from(buffer.1) * scale).round() as u32;
                    }
                    frame
                }
                None => [0, 0, crop_width, crop_height],
            };
            paneferry_protocol::dimensions(frame_width, frame_height)?;
            if crop_x > width as u32
                || crop_y > height as u32
                || left.saturating_add(frame_width) > crop_width
                || top.saturating_add(frame_height) > crop_height
                || crop_x.saturating_add(crop_width) > width as u32
                || crop_y.saturating_add(crop_height) > height as u32
            {
                bail!("invalid PipeWire crop")
            }
            let spec = CropSpec {
                source_width: width as u32,
                source_height: height as u32,
                region: [crop_x + left, crop_y + top, frame_width, frame_height],
            };
            let mut converter = self.converter.borrow_mut();
            if converter.as_ref().is_none_or(|c| c.spec != spec) {
                let mut next = CropConverter::new(self.api, spec)?;
                next.wake(self.wake.clone())?;
                *converter = Some(next);
            }
            // Convert directly into an independent I420 pool buffer. Only that
            // output is cached/queued; no cropped BGRx allocation is created.
            let converter = converter.as_ref().unwrap();
            converter.submit(buffer)?;
            converter.pull(0)
        })();
        if geometry_pending {
            // Static windows need not repaint after metadata catches up. Do
            // not lose their only resized frame or copy its pixel allocation.
            self.pending_geometry.replace(Some(sample));
        }
        result
    }
}
// Compare both forms of Mutter's crop metadata against the current logical
// geometry. The CPU-composited pixels remain at resource scale in either case;
// this corrects the crop rectangle, without scaling the image itself.
fn window_pixel_crop(
    frame: [u32; 4],
    buffer: (u32, u32),
    display_scale: f64,
    crop: (u32, u32),
) -> Option<[u32; 4]> {
    let pixels = |value: u32| (f64::from(value) * display_scale).round() as u32;
    if crop == (pixels(buffer.0), pixels(buffer.1)) || crop == buffer {
        return Some(frame.map(pixels));
    }
    None
}

impl Drop for PipeWireCapture {
    fn drop(&mut self) {
        self.pending_geometry.get_mut().take();
        unsafe {
            (self.api.state)(self.pipeline, STATE_NULL);
            if !self.sink.is_null() {
                if self.signal != 0 {
                    (self.api.signal_disconnect)(self.sink, self.signal);
                }
                (self.api.unref)(self.sink);
            }
            (self.api.unref)(self.pipeline);
        }
    }
}

// An input can outlive push/pull: keep its Vec alive until the last GstMemory
// reference is released. Only immutable, Send-owned Arc/Vec data crosses threads.
enum Pixels {
    Capture(Arc<CapturedImage>),
    Converted(Vec<u8>),
}
impl Pixels {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Capture(image) => &image.data[..image.pixels() * 4],
            Self::Converted(bytes) => bytes,
        }
    }
}
unsafe extern "C" fn release_pixels(pointer: Object) {
    // Allocated by Box::into_raw below; GStreamer invokes this exactly once.
    drop(unsafe { Box::from_raw(pointer.cast::<Pixels>()) });
}
pub struct Encoder {
    api: &'static Api,
    pipeline: Object,
    source: Object,
    sink: Object,
    pub width: u32,
    pub height: u32,
    pub stream: u64,
    pub format: Format,
    first_frame_fallback: Option<u8>,
}
pub struct Packet {
    pub bytes: Vec<u8>,
    pub keyframe: bool,
}

fn bitrate(width: u32, height: u32, fps: u8) -> u32 {
    (u64::from(width) * u64::from(height) * 6 * u64::from(fps) / 60).clamp(4_000_000, 48_000_000)
        as u32
}
#[derive(Clone, Copy)]
enum EncoderKind {
    X264,
    OpenH264,
}

#[cfg(test)]
fn pipeline(width: u32, height: u32, fps: u8, kind: EncoderKind) -> String {
    pipeline_for_format(width, height, fps, kind, Format::Bgrx)
}
fn pipeline_for_format(
    width: u32,
    height: u32,
    fps: u8,
    kind: EncoderKind,
    format: Format,
) -> String {
    let bitrate = bitrate(width, height, fps);
    let encoder = match kind {
        // Slice threads finish one frame at a time: no frame-thread/lookahead
        // buffering. Keep the same resolution, bitrate and maximum quantizer.
        EncoderKind::X264 => format!("x264enc name=encoder speed-preset=ultrafast tune=zerolatency \
            threads=4 sliced-threads=true bframes=0 rc-lookahead=0 sync-lookahead=0 \
            bitrate={} key-int-max={fps} qp-max=28 byte-stream=true", bitrate.div_ceil(1000)),
        EncoderKind::OpenH264 => format!("openh264enc name=encoder usage-type=screen complexity=low rate-control=bitrate \
            bitrate={bitrate} max-bitrate={} gop-size={fps} multi-thread=4 enable-frame-skip=false qp-max=28", bitrate * 2),
    };
    let padding = if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        format!("! videobox right=-{} bottom=-{} ", width % 2, height % 2)
    } else {
        String::new()
    };
    // Identity's datarate is a signed 32-bit integer. Timestamp after I420
    // conversion so even the maximum 8 MP frame at 90 fps fits that property.
    // Fixed per-frame timestamps keep rate control stable when capture is sparse.
    let even_width = u64::from(width + width % 2);
    let even_height = u64::from(height + height % 2);
    let raw_rate = even_width * even_height * 3 / 2 * u64::from(fps);
    let input = match format {
        Format::Bgrx => format!("caps=video/x-raw,format=BGRx,width={width},height={height},framerate={fps}/1 {padding}! videoconvert ! video/x-raw,format=I420"),
        Format::I420 => format!("caps=video/x-raw,format=I420,width={even_width},height={even_height},framerate={fps}/1"),
    };
    format!(
        "appsrc name=source is-live=true format=time block=true max-buffers=1 \
        {input} \
        ! identity datarate={raw_rate} signal-handoffs=false \
        ! {encoder} \
        ! h264parse config-interval=-1 ! video/x-h264,stream-format=byte-stream,alignment=au \
        ! appsink name=sink sync=false max-buffers=1 drop=false"
    )
}
impl Encoder {
    #[allow(dead_code)] // Used by the standalone quality_sample example.
    pub fn new(width: u32, height: u32, stream: u64) -> Result<Self> {
        Self::with_cap(width, height, stream, 60)
    }
    #[allow(dead_code)] // Used by the standalone quality_sample example.
    pub fn with_cap(width: u32, height: u32, stream: u64, cap_fps: u8) -> Result<Self> {
        Self::for_format(width, height, stream, cap_fps, Format::Bgrx)
    }
    pub fn for_format(
        width: u32,
        height: u32,
        stream: u64,
        cap_fps: u8,
        format: Format,
    ) -> Result<Self> {
        let api = api().context("H.264 components unavailable")?;
        if api.has_factory(c"x264enc") {
            if let Ok(mut encoder) =
                Self::with_kind_format(width, height, stream, cap_fps, EncoderKind::X264, format)
            {
                encoder.first_frame_fallback = Some(cap_fps);
                return Ok(encoder);
            }
        }
        Self::with_kind_format(
            width,
            height,
            stream,
            cap_fps,
            EncoderKind::OpenH264,
            format,
        )
    }
    #[cfg(test)]
    fn with_kind(
        width: u32,
        height: u32,
        stream: u64,
        cap_fps: u8,
        kind: EncoderKind,
    ) -> Result<Self> {
        Self::with_kind_format(width, height, stream, cap_fps, kind, Format::Bgrx)
    }
    fn with_kind_format(
        width: u32,
        height: u32,
        stream: u64,
        cap_fps: u8,
        kind: EncoderKind,
        format: Format,
    ) -> Result<Self> {
        paneferry_protocol::dimensions(width, height)?;
        anyhow::ensure!((1..=90).contains(&cap_fps), "invalid video fps");
        let api = api().context("H.264 components unavailable")?;
        let description = CString::new(pipeline_for_format(width, height, cap_fps, kind, format))?;
        let mut error = ptr::null_mut();
        let pipeline = unsafe { (api.parse)(description.as_ptr(), &mut error) };
        let mut encoder = Self {
            api,
            pipeline,
            source: ptr::null_mut(),
            sink: ptr::null_mut(),
            width,
            height,
            stream,
            format,
            first_frame_fallback: None,
        };
        if !error.is_null() {
            unsafe { (api.free_error)(error) };
            bail!("H.264 pipeline unavailable")
        }
        if pipeline.is_null() {
            bail!("H.264 pipeline unavailable")
        }
        unsafe {
            encoder.source = (api.get)(pipeline, c"source".as_ptr());
            encoder.sink = (api.get)(pipeline, c"sink".as_ptr());
            if encoder.source.is_null()
                || encoder.sink.is_null()
                || (api.state)(pipeline, STATE_PLAYING) == 0
            {
                bail!("H.264 pipeline unavailable")
            }
        }
        Ok(encoder)
    }
    #[allow(dead_code)] // Used by the standalone quality_sample example.
    pub fn encode(&mut self, image: &Arc<CapturedImage>) -> Result<Packet> {
        self.encode_frame(&Frame::from_image(image.clone()))
    }
    pub fn encode_frame(&mut self, image: &Frame) -> Result<Packet> {
        let fallback = self.first_frame_fallback.take();
        match self.encode_inner(image) {
            Ok(packet) => Ok(packet),
            Err(error) => {
                // Some installed plugins fail only during first-frame caps
                // negotiation. Retry on a fresh software pipeline before any
                // packet has entered the stream, never midway through a GOP.
                if let Some(fps) =
                    fallback.filter(|_| image.width == self.width && image.height == self.height)
                {
                    *self = Self::with_kind_format(
                        self.width,
                        self.height,
                        self.stream,
                        fps,
                        EncoderKind::OpenH264,
                        self.format,
                    )?;
                    self.encode_inner(image)
                } else {
                    Err(error)
                }
            }
        }
    }
    fn encode_inner(&mut self, image: &Frame) -> Result<Packet> {
        if image.width != self.width || image.height != self.height || image.format() != self.format
        {
            bail!("video geometry changed")
        }
        let buffer = image.input_buffer(self.api)?;
        // Unlike the action signal, C appsrc consumes this reference even on error.
        if unsafe { (self.api.push)(self.source, buffer) } != 0 {
            bail!("H.264 input rejected")
        }
        let sample = unsafe { (self.api.pull)(self.sink, 2_000_000_000) };
        if sample.is_null() {
            bail!("H.264 output timeout")
        }
        struct Sample(Object, &'static Api);
        impl Drop for Sample {
            fn drop(&mut self) {
                unsafe { (self.1.sample_unref)(self.0) }
            }
        }
        let sample = Sample(sample, self.api);
        let encoded = unsafe { (self.api.sample_buffer)(sample.0) };
        if encoded.is_null() {
            bail!("missing H.264 buffer")
        }
        let length = unsafe { (self.api.size)(encoded) };
        if length == 0 || length > 12 * 1024 * 1024 {
            bail!("invalid encoded video packet")
        }
        let mut bytes = vec![0; length];
        if unsafe { (self.api.extract)(encoded, 0, bytes.as_mut_ptr().cast(), length) } != length {
            bail!("incomplete H.264 buffer")
        }
        let keyframe = unsafe { (self.api.flags)(encoded) } & BUFFER_DELTA_UNIT == 0;
        Ok(Packet { bytes, keyframe })
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            if !self.pipeline.is_null() {
                (self.api.state)(self.pipeline, STATE_NULL);
            }
            if !self.source.is_null() {
                (self.api.unref)(self.source);
            }
            if !self.sink.is_null() {
                (self.api.unref)(self.sink);
            }
            if !self.pipeline.is_null() {
                (self.api.unref)(self.pipeline);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        os::{fd::OwnedFd, unix::net::UnixStream},
        process::{Command, Stdio},
        thread,
        time::Duration,
    };

    #[test]
    fn logical_crop_metadata_preserves_hidpi_pixels_and_rejects_stale_sizes() {
        let frame = [26, 26, 700, 450];
        assert_eq!(
            window_pixel_crop(frame, (752, 502), 2.0, (1504, 1004)),
            Some([52, 52, 1400, 900])
        );
        assert_eq!(
            window_pixel_crop(frame, (752, 502), 2.0, (752, 502)),
            Some([52, 52, 1400, 900])
        );
        assert_eq!(window_pixel_crop(frame, (752, 502), 2.0, (1504, 502)), None);
        assert_eq!(window_pixel_crop(frame, (752, 502), 2.0, (1470, 809)), None);
        assert_eq!(
            window_pixel_crop(frame, (752, 502), 1.5, (1128, 753)),
            Some([39, 39, 1050, 675])
        );
    }

    #[test]
    fn crop_copies_exact_pixels_without_padding_or_aliasing() {
        let mut pixels: Vec<u8> = (0..6 * 5 * 4).map(|n| n as u8).collect();
        let cropped = copy_bgrx_crop(&pixels, 6, [2, 1, 3, 2]).unwrap();
        assert_eq!(
            cropped,
            [pixels[32..44].to_vec(), pixels[56..68].to_vec()].concat()
        );
        let full_rows = copy_bgrx_crop(&pixels, 6, [0, 2, 6, 2]).unwrap();
        assert_eq!(full_rows, pixels[48..96]);
        pixels.fill(0);
        assert_eq!(cropped[0], 32);
        assert_eq!(full_rows[0], 48);
        for crop in [
            [4, 0, 3, 1],
            [0, 4, 6, 2],
            [0, 0, 0, 1],
            [0, usize::MAX, 1, 2],
        ] {
            assert!(copy_bgrx_crop(&pixels, 6, crop).is_err());
        }
        assert!(copy_bgrx_crop(&pixels[..47], 6, [0, 1, 6, 1]).is_err());
    }

    #[test]
    fn frame_wake_coalesces_and_rearms_without_losing_concurrent_notifications() {
        let wake = Arc::new(FrameWake::new().unwrap());
        let ready = |wake: &FrameWake| {
            let mut fd = libc::pollfd {
                fd: wake.event_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            (unsafe { libc::poll(&mut fd, 1, 0) }) > 0
        };
        assert!(!ready(&wake));
        for _ in 0..100 {
            wake.notify();
        }
        assert!(ready(&wake));
        wake.drain();
        assert!(!ready(&wake));
        let other = wake.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let producer = thread::spawn(move || {
            for _ in 0..1_000 {
                other.notify();
            }
            rx.recv().unwrap();
            other.notify();
        });
        for _ in 0..50 {
            wake.drain();
        }
        tx.send(()).unwrap();
        producer.join().unwrap();
        assert!(ready(&wake));
        wake.drain();
        assert!(!ready(&wake));
        wake.notify();
        assert!(ready(&wake));
    }

    #[test]
    #[ignore = "requires Linux GStreamer videotestsrc; run explicitly on Ubuntu"]
    fn appsink_new_sample_wakes_poll_without_periodic_frame_scans() {
        let wake = Arc::new(FrameWake::new().unwrap());
        let description = c"videotestsrc is-live=true ! video/x-raw,format=BGRx,width=64,height=48,framerate=30/1 ! appsink name=frames max-buffers=1 drop=true sync=false emit-signals=true enable-last-sample=false";
        let capture = PipeWireCapture::from_description(description, wake.clone()).unwrap();
        let mut event = libc::pollfd {
            fd: wake.event_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(unsafe { libc::poll(&mut event, 1, 1000) }, 1);
        wake.drain();
        let image = loop {
            if let Some(image) = capture.pull_monitor().unwrap() {
                break image;
            }
            assert_eq!(unsafe { libc::poll(&mut event, 1, 1000) }, 1);
            wake.drain();
        };
        assert_eq!((image.width, image.height), (64, 48));
        drop(capture);
        // Pixels remain owned and usable after mapping, sample and pipeline die.
        assert_eq!(image.format(), Format::I420);
        let buffer = image.input_buffer(api().unwrap()).unwrap();
        assert_eq!(unsafe { (api().unwrap().size)(buffer) }, 64 * 48 * 3 / 2);
        unsafe { (api().unwrap().mini_unref)(buffer) };
        wake.drain();
    }

    #[test]
    #[ignore = "requires Linux GStreamer; ready capture samples must not wait on presentation timestamps"]
    fn raw_capture_does_not_wait_for_source_presentation_clock() {
        let started = std::time::Instant::now();
        let capture = PipeWireCapture::from_description(
            c"videotestsrc num-buffers=2 is-live=true ! video/x-raw,format=BGRx,width=64,height=48,framerate=1/1 ! appsink name=frames max-buffers=1 drop=false sync=false enable-last-sample=false",
            Arc::new(FrameWake::new().unwrap()),
        ).unwrap();
        for _ in 0..2 {
            let sample = unsafe { (capture.api.pull)(capture.sink, 2_000_000_000) };
            assert!(!sample.is_null());
            unsafe { (capture.api.sample_unref)(sample) };
        }
        // With the default source clock, the second timestamp waits one second
        // even though appsink sync=false. PipeWire itself remains event driven;
        // this synthetic producer isolates that additional clock wait.
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    #[ignore = "requires Linux GStreamer; a static resize frame must survive late geometry metadata"]
    fn resized_sample_survives_late_geometry_without_another_repaint() {
        let api = api().unwrap();
        let wake = Arc::new(FrameWake::new().unwrap());
        let capture = PipeWireCapture::from_description(
            c"appsrc name=input is-live=true format=time caps=video/x-raw,format=BGRx,width=64,height=48,framerate=60/1 ! appsink name=frames max-buffers=1 drop=true sync=false emit-signals=true enable-last-sample=false",
            wake.clone(),
        ).unwrap();
        let source = unsafe { (api.get)(capture.pipeline, c"input".as_ptr()) };
        assert!(!source.is_null());
        // gst_buffer_add_video_crop_meta is a public header macro over these
        // two exported functions, rather than an exported function itself.
        let (add_meta, crop_info): (
            unsafe extern "C" fn(Object, *const c_void, Object) -> *mut VideoCropMeta,
            unsafe extern "C" fn() -> *const c_void,
        ) = unsafe {
            let core = libc::dlopen(c"libgstreamer-1.0.so.0".as_ptr(), libc::RTLD_NOW);
            let video = libc::dlopen(c"libgstvideo-1.0.so.0".as_ptr(), libc::RTLD_NOW);
            assert!(!core.is_null() && !video.is_null());
            let add = libc::dlsym(core, c"gst_buffer_add_meta".as_ptr());
            let info = libc::dlsym(video, c"gst_video_crop_meta_get_info".as_ptr());
            assert!(!add.is_null() && !info.is_null());
            (std::mem::transmute(add), std::mem::transmute(info))
        };
        let mut ready = libc::pollfd {
            fd: wake.event_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let mut originals = Vec::new();
        let mut expected = None;
        // A newer incompatible sample replaces the retained one, rather than
        // accumulating producer leases while compositor metadata catches up.
        for value in [64, 192] {
            let original = Arc::new(CapturedImage {
                width: 64,
                height: 48,
                data: vec![value; 64 * 48 * 4],
                bpp: 32,
                pad: 32,
                little: true,
                masks: (0xff0000, 0xff00, 0xff),
            });
            let buffer = Frame::from_image(original.clone())
                .input_buffer(api)
                .unwrap();
            let converter = CropConverter::new(
                api,
                CropSpec {
                    source_width: 64,
                    source_height: 48,
                    region: [0, 0, 64, 48],
                },
            )
            .unwrap();
            expected = Some(converter.convert(buffer).unwrap());
            drop(converter);
            unsafe {
                let crop = add_meta(buffer, crop_info(), ptr::null_mut());
                assert!(!crop.is_null());
                (*crop).x = 0;
                (*crop).y = 0;
                (*crop).width = 64;
                (*crop).height = 48;
                assert_eq!((api.push)(source, buffer), 0);
            }
            assert_eq!(unsafe { libc::poll(&mut ready, 1, 1000) }, 1);
            wake.drain();
            assert!(capture
                .pull([0, 0, 128, 96], (128, 96), 1.0)
                .unwrap()
                .is_none());
            originals.push(original);
        }
        unsafe { (api.unref)(source) };
        // No more input buffers or repaint requests. Only metadata changes.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let frame = loop {
            if let Some(frame) = capture.pull([0, 0, 64, 48], (64, 48), 1.0).unwrap() {
                break frame;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "resize frame was lost before metadata arrived"
            );
            unsafe { libc::poll(&mut ready, 1, 20) };
            wake.drain();
        };
        assert!(
            frame.same_pixels(&expected.unwrap()),
            "must retain the latest sample"
        );
        drop(capture);
        assert!(originals.iter().all(|image| Arc::strong_count(image) == 1));
    }

    #[test]
    fn ninety_fps_timestamps_fit_identity_datarate_at_maximum_resolution() {
        let description = pipeline(4000, 2000, 90, EncoderKind::X264);
        assert!(description.contains("framerate=90/1"));
        assert!(description.contains("datarate=1080000000"));
        assert!(1_080_000_000 < i32::MAX);
    }

    #[test]
    #[ignore = "requires Linux GStreamer x264/OpenH264; run explicitly on Ubuntu"]
    fn first_frame_failure_falls_back_without_changing_stream_or_retaining_pixels() {
        let image = Arc::new(CapturedImage {
            width: 128,
            height: 64,
            data: vec![96; 128 * 64 * 4],
            bpp: 32,
            pad: 32,
            little: true,
            masks: (0xff0000, 0xff00, 0xff),
        });
        let mut encoder = Encoder::with_kind(128, 64, 7, 60, EncoderKind::X264).unwrap();
        encoder.first_frame_fallback = Some(60);
        // Simulate a plugin that was found/constructed but cannot accept input.
        unsafe {
            (encoder.api.state)(encoder.pipeline, STATE_NULL);
        }
        let first = encoder.encode(&image).unwrap();
        assert!(first.keyframe);
        assert_eq!(encoder.stream, 7);
        assert!(encoder.first_frame_fallback.is_none());
        assert!(!encoder.encode(&image).unwrap().keyframe);
        drop(encoder);
        assert_eq!(Arc::strong_count(&image), 1);
    }

    #[test]
    #[ignore = "requires Linux GStreamer/OpenH264 and Python GI; run explicitly on Ubuntu"]
    fn native_encoder_preserves_pixels_and_releases_capture_leases() {
        assert!(available());
        const FRAMES: usize = 3;
        for (width, height) in [(640, 480), (641, 483), (2940, 1612)] {
            let mut base = Vec::with_capacity(width as usize * height as usize * 4);
            for y in 0..height {
                for x in 0..width {
                    let ink = (x % 19 < 2 || y % 23 < 2) && x % 211 < 120;
                    let pixel = if ink {
                        [8, 8, 8, 0]
                    } else {
                        [(x % 251) as u8, (y % 239) as u8, 210, 0]
                    };
                    base.extend_from_slice(&pixel);
                }
            }
            let (mut wire, child) = UnixStream::pair().unwrap();
            wire.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            wire.set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let process = Command::new("/usr/bin/python3")
                .args([
                    "-u",
                    "-c",
                    include_str!("../../../tests/performance/legacy_video_encoder.py"),
                    &width.to_string(),
                    &height.to_string(),
                ])
                .stdin(Stdio::from(OwnedFd::from(child.try_clone().unwrap())))
                .stdout(Stdio::from(OwnedFd::from(child)))
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            struct Worker(std::process::Child);
            impl Drop for Worker {
                fn drop(&mut self) {
                    let _ = self.0.kill();
                    let _ = self.0.wait();
                }
            }
            let worker = Worker(process);
            let mut encoder = Encoder::new(width, height, 1).unwrap();
            let (mut legacy, mut native, mut originals, mut images) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for frame in 0..FRAMES {
                let mut data = base.clone();
                for y in 20..84 {
                    for x in 20..84 {
                        let offset = (y * width as usize + x) * 4;
                        data[offset..offset + 4].copy_from_slice(&[0, (frame * 70) as u8, 255, 0]);
                    }
                }
                let image = Arc::new(CapturedImage {
                    width,
                    height,
                    data,
                    bpp: 32,
                    pad: 32,
                    little: true,
                    masks: (0xff0000, 0xff00, 0xff),
                });
                let rgb: Vec<_> = image
                    .data
                    .chunks_exact(4)
                    .flat_map(|p| [p[2], p[1], p[0]])
                    .collect();
                let packet = encoder.encode(&image).unwrap();
                assert_eq!(packet.keyframe, frame == 0);
                native.extend_from_slice(&packet.bytes);
                assert!(
                    image
                        .data
                        .chunks_exact(4)
                        .flat_map(|p| [p[2], p[1], p[0]])
                        .eq(rgb.iter().copied()),
                    "source pixels mutated"
                );
                originals.extend_from_slice(&rgb);
                wire.write_all(&(image.data.len() as u32).to_be_bytes())
                    .unwrap();
                wire.write_all(&image.data).unwrap();
                let mut header = [0; 5];
                wire.read_exact(&mut header).unwrap();
                let length = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
                assert!(length > 0 && length <= 12 * 1024 * 1024);
                let mut bytes = vec![0; length];
                wire.read_exact(&mut bytes).unwrap();
                legacy.extend_from_slice(&bytes);
                images.push(image);
            }
            drop(worker);
            drop(encoder);
            for image in &images {
                assert_eq!(
                    Arc::strong_count(image),
                    1,
                    "input lease retained after pipeline shutdown"
                );
            }
            let decode = |bytes: &[u8]| {
                let mut child = Command::new("/usr/bin/python3")
                    .args([
                        "-c",
                        include_str!("../../../tests/performance/decode_quality.py"),
                        &width.to_string(),
                        &height.to_string(),
                        &FRAMES.to_string(),
                    ])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                child.stdin.take().unwrap().write_all(bytes).unwrap();
                let output = child.wait_with_output().unwrap();
                assert!(output.status.success());
                assert_eq!(output.stdout.len(), originals.len());
                output.stdout
            };
            let old = decode(&legacy);
            let new = decode(&native);
            let frame_size = width as usize * height as usize * 3;
            for frame in 0..FRAMES {
                let start = frame * frame_size;
                let end = start + frame_size;
                let reference = &originals[start..end];
                let mse = |pixels: &[u8]| {
                    pixels
                        .iter()
                        .zip(reference)
                        .map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2))
                        .sum::<f64>()
                        / pixels.len() as f64
                };
                let old_mse = mse(&old[start..end]);
                let new_mse = mse(&new[start..end]);
                eprintln!("quality {width}x{height} frame {frame}: legacy MSE={old_mse:.4}, native MSE={new_mse:.4}, identical={}",old[start..end]==new[start..end]);
                assert!(
                    new_mse <= old_mse * 1.02 + 0.01,
                    "decoded fidelity regressed"
                );
            }
        }
    }
}
