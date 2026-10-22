//! Runtime-loaded GStreamer C API: share captured pixels, without a raw-frame subprocess.
use anyhow::{bail, Context, Result};
use paneferry_backend_x11::CapturedImage;
use std::{
    borrow::Cow,
    ffi::{c_char, c_int, c_uint, c_void, CStr, CString},
    ptr,
    sync::{Arc, OnceLock},
};

type Object = *mut c_void;
type Destroy = unsafe extern "C" fn(Object);
const MEMORY_READONLY: c_uint = 1 << 1;
const BUFFER_DELTA_UNIT: c_uint = 1 << 13;
const STATE_NULL: c_int = 1;
const STATE_PLAYING: c_int = 4;

// Concrete public C signatures and opaque pointers only: no GObject/GstBuffer
// layout assumptions. Libraries intentionally stay loaded for process-global
// plugin state and destroy callbacks running on GStreamer streaming threads.
macro_rules! runtime_api {
    ($core:ident, $app:ident, $glib:ident; $($field:ident: $signature:ty => ($library:ident, $symbol:expr)),+ $(,)?) => {
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
    core, app, glib;
    init: unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char, *mut Object) -> c_int => (core, c"gst_init_check"),
    factory: unsafe extern "C" fn(*const c_char) -> Object => (core, c"gst_element_factory_find"),
    parse: unsafe extern "C" fn(*const c_char, *mut Object) -> Object => (core, c"gst_parse_launch"),
    get: unsafe extern "C" fn(Object, *const c_char) -> Object => (core, c"gst_bin_get_by_name"),
    state: unsafe extern "C" fn(Object, c_int) -> c_int => (core, c"gst_element_set_state"),
    unref: unsafe extern "C" fn(Object) -> () => (core, c"gst_object_unref"),
    free_error: unsafe extern "C" fn(Object) -> () => (glib, c"g_error_free"),
    wrap: unsafe extern "C" fn(c_uint, Object, usize, usize, usize, Object, Option<Destroy>) -> Object => (core, c"gst_buffer_new_wrapped_full"),
    push: unsafe extern "C" fn(Object, Object) -> c_int => (app, c"gst_app_src_push_buffer"),
    pull: unsafe extern "C" fn(Object, u64) -> Object => (app, c"gst_app_sink_try_pull_sample"),
    sample_buffer: unsafe extern "C" fn(Object) -> Object => (core, c"gst_sample_get_buffer"),
    sample_unref: unsafe extern "C" fn(Object) -> () => (core, c"gst_sample_unref"),
    size: unsafe extern "C" fn(Object) -> usize => (core, c"gst_buffer_get_size"),
    extract: unsafe extern "C" fn(Object, usize, Object, usize) -> usize => (core, c"gst_buffer_extract"),
    flags: unsafe extern "C" fn(Object) -> c_uint => (core, c"gst_buffer_get_flags"),
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
        for name in [
            c"appsrc",
            c"identity",
            c"videoconvert",
            c"videobox",
            c"openh264enc",
            c"h264parse",
            c"appsink",
        ] {
            let factory = unsafe { (self.factory)(name.as_ptr()) };
            if factory.is_null() {
                bail!("H.264 components unavailable")
            }
            unsafe { (self.unref)(factory) };
        }
        Ok(())
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
    api().is_some()
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
}
pub struct Packet {
    pub bytes: Vec<u8>,
    pub keyframe: bool,
}

fn pipeline(width: u32, height: u32) -> String {
    let bitrate = (width * height * 3).clamp(2_000_000, 24_000_000);
    let padding = if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        format!("! videobox right=-{} bottom=-{} ", width % 2, height % 2)
    } else {
        String::new()
    };
    // Retain sequence/30 timestamps via a metadata-only identity element, rather
    // than changing rate-control timing to variable wall-clock capture intervals.
    let raw_rate = width * height * 4 * 30;
    format!("appsrc name=source is-live=true format=time block=true max-buffers=1 \
        caps=video/x-raw,format=BGRx,width={width},height={height},framerate=30/1 \
        ! identity datarate={raw_rate} signal-handoffs=false \
        {padding}! videoconvert ! video/x-raw,format=I420 \
        ! openh264enc usage-type=screen complexity=low rate-control=bitrate \
        bitrate={bitrate} max-bitrate={} gop-size=60 multi-thread=4 enable-frame-skip=false qp-max=28 \
        ! h264parse config-interval=-1 ! video/x-h264,stream-format=byte-stream,alignment=au \
        ! appsink name=sink sync=false max-buffers=1 drop=false", bitrate * 2)
}
impl Encoder {
    pub fn new(width: u32, height: u32, stream: u64) -> Result<Self> {
        paneferry_protocol::dimensions(width, height)?;
        let api = api().context("H.264 components unavailable")?;
        let description = CString::new(pipeline(width, height))?;
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
    pub fn encode(&mut self, image: &Arc<CapturedImage>) -> Result<Packet> {
        if image.width != self.width || image.height != self.height {
            bail!("video geometry changed")
        }
        let pixels = Box::new(match image.bgrx()? {
            Cow::Borrowed(_) => Pixels::Capture(image.clone()),
            Cow::Owned(bytes) => Pixels::Converted(bytes),
        });
        let bytes = pixels.bytes();
        let length = bytes.len();
        let data = bytes.as_ptr().cast_mut().cast::<c_void>();
        let owner = Box::into_raw(pixels).cast::<c_void>();
        // READONLY forbids in-place mutation, so the capture cache can safely
        // compare the same original bytes while GStreamer shares their storage.
        let buffer = unsafe {
            (self.api.wrap)(
                MEMORY_READONLY,
                data,
                length,
                0,
                length,
                owner,
                Some(release_pixels),
            )
        };
        if buffer.is_null() {
            unsafe { release_pixels(owner) };
            bail!("cannot wrap video frame")
        }
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
        time::Duration,
    };

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
