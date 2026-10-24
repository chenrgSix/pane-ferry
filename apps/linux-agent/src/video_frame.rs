//! Immutable frames shared by capture, deduplication and the encoder. Native
//! capture converts the selected source rectangle directly into owned I420.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Bgrx,
    I420,
}

pub struct Frame {
    pub width: u32,
    pub height: u32,
    storage: Storage,
}
enum Storage {
    Image(Arc<CapturedImage>),
    I420(Sample),
}
struct Sample(Object, &'static Api);
impl Drop for Sample {
    fn drop(&mut self) {
        unsafe { (self.1.sample_unref)(self.0) };
    }
}
// GStreamer refcounted samples/buffers are thread safe. After appsink hands us
// the sample we only map READ or give appsrc an additional buffer reference;
// writable downstream operations must obtain their own writable buffer.
unsafe impl Send for Sample {}
unsafe impl Sync for Sample {}

impl From<CapturedImage> for Frame {
    fn from(image: CapturedImage) -> Self {
        Self::from_image(Arc::new(image))
    }
}
impl Frame {
    pub fn from_image(image: Arc<CapturedImage>) -> Self {
        Self {
            width: image.width,
            height: image.height,
            storage: Storage::Image(image),
        }
    }
    pub fn pixels(&self) -> usize {
        self.width as usize * self.height as usize
    }
    pub fn format(&self) -> Format {
        match self.storage {
            Storage::Image(_) => Format::Bgrx,
            Storage::I420(_) => Format::I420,
        }
    }
    pub fn same_pixels(&self, other: &Self) -> bool {
        if (self.width, self.height) != (other.width, other.height) {
            return false;
        }
        match (&self.storage, &other.storage) {
            (Storage::Image(a), Storage::Image(b)) => a == b,
            (Storage::I420(a), Storage::I420(b)) => {
                let compare = || -> Result<bool> {
                    let a = ReadMap::new(a.1, unsafe { (a.1.sample_buffer)(a.0) })?;
                    let b = ReadMap::new(b.1, unsafe { (b.1.sample_buffer)(b.0) })?;
                    let (a, b) = (a.bytes()?, b.bytes()?);
                    // Ignore alignment bytes, which a pool may leave untouched.
                    let mut offset = 0;
                    let w = (self.width + self.width % 2) as usize;
                    let h = (self.height + self.height % 2) as usize;
                    for (width, height) in [(w, h), (w / 2, h / 2), (w / 2, h / 2)] {
                        let stride = width.next_multiple_of(4);
                        for row in 0..height {
                            let start = offset + row * stride;
                            if a.get(start..start + width).context("short I420 frame")?
                                != b.get(start..start + width).context("short I420 frame")?
                            {
                                return Ok(false);
                            }
                        }
                        offset += stride * height;
                    }
                    Ok(true)
                };
                compare().unwrap_or(false)
            }
            _ => false,
        }
    }
    /// An owned reference, consumed by gst_app_src_push_buffer even on error.
    pub(super) fn input_buffer(&self, api: &'static Api) -> Result<Object> {
        match &self.storage {
            Storage::I420(sample) => Ok(unsafe { (api.mini_ref)((api.sample_buffer)(sample.0)) }),
            Storage::Image(image) => {
                let pixels = Box::new(match image.bgrx()? {
                    Cow::Borrowed(_) => Pixels::Capture(image.clone()),
                    Cow::Owned(bytes) => Pixels::Converted(bytes),
                });
                let bytes = pixels.bytes();
                let length = bytes.len();
                let data = bytes.as_ptr().cast_mut().cast();
                let owner = Box::into_raw(pixels).cast();
                let buffer = unsafe {
                    (api.wrap)(
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
                Ok(buffer)
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct CropSpec {
    pub source_width: u32,
    pub source_height: u32,
    pub region: [u32; 4],
}
pub(super) struct CropConverter {
    pub spec: CropSpec,
    api: &'static Api,
    pipeline: Object,
    source: Object,
    sink: Object,
    signal: libc::c_ulong,
}
impl CropConverter {
    pub fn new(api: &'static Api, spec: CropSpec) -> Result<Self> {
        let [x, y, width, height] = spec.region;
        paneferry_protocol::dimensions(width, height)?;
        if x.checked_add(width).is_none_or(|n| n > spec.source_width)
            || y.checked_add(height).is_none_or(|n| n > spec.source_height)
        {
            bail!("invalid PipeWire crop")
        }
        // 1.28.2 does not consume CropMeta itself. Explicit converter regions
        // work there too. Destination pixels are never scaled; odd dimensions
        // use the converter's black border in the even-sized I420 allocation.
        let description = CString::new(format!(
            "appsrc name=source is-live=true format=time block=false max-buffers=1 leaky-type=downstream \
             caps=video/x-raw,format=BGRx,width={},height={},framerate=60/1 \
             ! videoconvertscale converter-config=\"GstVideoConverter, \
             GstVideoConverter.src-x=(int){x}, GstVideoConverter.src-y=(int){y}, \
             GstVideoConverter.src-width=(int){width}, GstVideoConverter.src-height=(int){height}, \
             GstVideoConverter.dest-width=(int){width}, GstVideoConverter.dest-height=(int){height}\" \
             ! video/x-raw,format=I420,width={},height={} \
             ! appsink name=sink sync=false max-buffers=1 drop=true enable-last-sample=false emit-signals=true",
            spec.source_width, spec.source_height, width + width % 2, height + height % 2))?;
        Self::from_description(api, spec, description)
    }
    fn from_description(api: &'static Api, spec: CropSpec, description: CString) -> Result<Self> {
        let mut error = ptr::null_mut();
        let pipeline = unsafe { (api.parse)(description.as_ptr(), &mut error) };
        let mut converter = Self {
            spec,
            api,
            pipeline,
            source: ptr::null_mut(),
            sink: ptr::null_mut(),
            signal: 0,
        };
        if !error.is_null() {
            unsafe { (api.free_error)(error) };
            bail!("PipeWire color conversion unavailable")
        }
        if pipeline.is_null() {
            bail!("PipeWire color conversion unavailable")
        }
        unsafe {
            converter.source = (api.get)(pipeline, c"source".as_ptr());
            converter.sink = (api.get)(pipeline, c"sink".as_ptr());
            if converter.source.is_null()
                || converter.sink.is_null()
                || (api.state)(pipeline, STATE_PLAYING) == 0
            {
                bail!("PipeWire color conversion unavailable")
            }
        }
        Ok(converter)
    }
    pub fn wake(&mut self, wake: Arc<FrameWake>) -> Result<()> {
        self.signal = connect_wake(self.api, self.sink, wake)?;
        Ok(())
    }
    #[cfg(test)]
    pub fn convert(&self, buffer: Object) -> Result<Frame> {
        self.submit(buffer)?;
        self.pull(2_000_000_000)?
            .context("PipeWire color conversion timeout")
    }
    pub fn submit(&self, buffer: Object) -> Result<()> {
        // Copy only the buffer header, sharing the original packed BGRx memory.
        // FLAGS | TIMESTAMPS | MEMORY deliberately excludes META: the corrected
        // crop is already in converter-config, including HiDPI and shadow bounds.
        // Do not let a newer plugin apply the original logical CropMeta again.
        let input = unsafe { (self.api.copy_region)(buffer, 1 | 2 | 8, 0, usize::MAX) };
        if input.is_null() {
            bail!("cannot reference PipeWire frame")
        }
        // Sharing GstMemory alone does not keep a pool-owned source buffer from
        // being recycled. Hold its parent buffer until conversion consumes it.
        if unsafe { (self.api.parent_buffer)(input, buffer) }.is_null() {
            unsafe { (self.api.mini_unref)(input) };
            bail!("cannot retain PipeWire buffer lease")
        }
        if unsafe { (self.api.size)(input) }
            != self.spec.source_width as usize * self.spec.source_height as usize * 4
        {
            unsafe { (self.api.mini_unref)(input) };
            bail!("invalid PipeWire conversion stride")
        }
        if unsafe { (self.api.push)(self.source, input) } != 0 {
            bail!("PipeWire conversion input rejected")
        }
        Ok(())
    }
    pub fn pull(&self, timeout: u64) -> Result<Option<Frame>> {
        let sample = unsafe { (self.api.pull)(self.sink, timeout) };
        if sample.is_null() {
            return Ok(None);
        }
        let sample = Sample(sample, self.api);
        let output = unsafe { (self.api.sample_buffer)(sample.0) };
        let [_, _, width, height] = self.spec.region;
        if output.is_null() {
            bail!("missing converted frame")
        }
        let w = (width + width % 2) as usize;
        let h = (height + height % 2) as usize;
        let expected = w.next_multiple_of(4) * h + (w / 2).next_multiple_of(4) * h;
        if unsafe { (self.api.size)(output) } != expected {
            bail!("invalid converted frame layout")
        }
        Ok(Some(Frame {
            width,
            height,
            storage: Storage::I420(sample),
        }))
    }
}
impl Drop for CropConverter {
    fn drop(&mut self) {
        unsafe {
            if !self.pipeline.is_null() {
                (self.api.state)(self.pipeline, STATE_NULL);
            }
            if !self.source.is_null() {
                (self.api.unref)(self.source);
            }
            if !self.sink.is_null() {
                if self.signal != 0 {
                    (self.api.signal_disconnect)(self.sink, self.signal);
                }
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
    use std::time::Instant;

    struct Buffer(Object);
    impl Drop for Buffer {
        fn drop(&mut self) {
            unsafe { (api().unwrap().mini_unref)(self.0) };
        }
    }
    fn image(width: u32, height: u32, phase: u32) -> CapturedImage {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let ink = ((y + phase * 7) % 31 < 3 || (x + phase * 13) % 19 < 2) && x % 211 < 120;
                data.extend_from_slice(if ink {
                    &[8, 8, 8, 0]
                } else {
                    &[180, 195, 210, 0]
                });
            }
        }
        CapturedImage {
            width,
            height,
            data,
            bpp: 32,
            pad: 32,
            little: true,
            masks: (0xff0000, 0xff00, 0xff),
        }
    }
    fn buffer(image: &Arc<CapturedImage>) -> Buffer {
        // A compositor backing buffer may exceed the protocol's visible pixel
        // budget (e.g. 3840x2160 contains a smaller 2940x1618 window).
        let api = api().unwrap();
        let data = image.data.as_ptr().cast_mut().cast();
        let length = image.data.len();
        let owner = Box::into_raw(Box::new(Pixels::Capture(image.clone()))).cast();
        let buffer = unsafe {
            (api.wrap)(
                MEMORY_READONLY,
                data,
                length,
                0,
                length,
                owner,
                Some(release_pixels),
            )
        };
        assert!(!buffer.is_null());
        Buffer(buffer)
    }
    fn cropped(source: &CapturedImage, region: [u32; 4]) -> CapturedImage {
        let data = copy_bgrx_crop(
            &source.data,
            source.width as usize,
            region.map(|n| n as usize),
        )
        .unwrap();
        CapturedImage {
            width: region[2],
            height: region[3],
            data,
            bpp: 32,
            pad: 32,
            little: true,
            masks: (0xff0000, 0xff00, 0xff),
        }
    }
    fn reference(width: u32, height: u32) -> CropConverter {
        let padding = if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            format!("! videobox right=-{} bottom=-{}", width % 2, height % 2)
        } else {
            String::new()
        };
        let description = CString::new(format!(
            "appsrc name=source is-live=true format=time block=true max-buffers=1 caps=video/x-raw,format=BGRx,width={width},height={height},framerate=60/1 \
             {padding} ! videoconvert ! video/x-raw,format=I420 ! appsink name=sink sync=false max-buffers=1 drop=false enable-last-sample=false")).unwrap();
        CropConverter::from_description(
            api().unwrap(),
            CropSpec {
                source_width: width,
                source_height: height,
                region: [0, 0, width, height],
            },
            description,
        )
        .unwrap()
    }

    fn assert_reference_pixels(actual: &Frame, expected: &Frame) {
        let api = api().unwrap();
        let a = Buffer(actual.input_buffer(api).unwrap());
        let b = Buffer(expected.input_buffer(api).unwrap());
        let ma = ReadMap::new(api, a.0).unwrap();
        let mb = ReadMap::new(api, b.0).unwrap();
        let (a, b) = (ma.bytes().unwrap(), mb.bytes().unwrap());
        let w = (actual.width + actual.width % 2) as usize;
        let h = (actual.height + actual.height % 2) as usize;
        let mut offset = 0;
        for (plane, (width, height)) in [(w, h), (w / 2, h / 2), (w / 2, h / 2)]
            .into_iter()
            .enumerate()
        {
            let stride = width.next_multiple_of(4);
            for y in 0..height {
                for x in 0..width {
                    let index = offset + y * stride + x;
                    // RGB black padding before subsampling (old) and I420 border
                    // fill (new) may differ only at the odd-size chroma edge.
                    let edge = plane > 0
                        && ((!actual.width.is_multiple_of(2) && x == width - 1)
                            || (!actual.height.is_multiple_of(2) && y == height - 1));
                    assert!(
                        a[index].abs_diff(b[index]) <= if edge { 64 } else { 0 },
                        "I420 mismatch {}x{} plane={plane} at ({x},{y}): {} vs {}",
                        actual.width,
                        actual.height,
                        a[index],
                        b[index]
                    );
                }
            }
            offset += stride * height;
        }
    }

    #[test]
    #[ignore = "requires Linux GStreamer; compares actual I420 against the previous crop/convert path"]
    fn native_crop_matches_reference_and_releases_source_before_encoding() {
        let api = api().unwrap();
        for (width, height) in [(64, 48), (641, 480), (640, 483), (641, 483), (2940, 1618)] {
            let region = [7, 5, width, height];
            let original = Arc::new(image(width + 40, height + 30, 0));
            let source = buffer(&original);
            let converter = CropConverter::new(
                api,
                CropSpec {
                    source_width: original.width,
                    source_height: original.height,
                    region,
                },
            )
            .unwrap();
            let converted = converter.convert(source.0).unwrap();
            let same = converter.convert(source.0).unwrap();
            assert!(converted.same_pixels(&same));
            let packed = Arc::new(cropped(&original, region));
            let packed_buffer = buffer(&packed);
            let expected = reference(width, height).convert(packed_buffer.0).unwrap();
            assert_reference_pixels(&converted, &expected);
            let changed_image = Arc::new(image(original.width, original.height, 1));
            let changed_buffer = buffer(&changed_image);
            let changed = converter.convert(changed_buffer.0).unwrap();
            assert!(!converted.same_pixels(&changed));
            drop(source);
            // A previous source must not be retained by cached converted frames.
            assert_eq!(Arc::strong_count(&original), 1);
            drop(changed_buffer);
            let released_by = Instant::now() + std::time::Duration::from_secs(1);
            while Arc::strong_count(&changed_image) != 1 && Instant::now() < released_by {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert_eq!(Arc::strong_count(&changed_image), 1);
            drop(converter);
            assert_reference_pixels(&converted, &expected);
            // Exercise both codecs and a forced first-frame fallback for I420.
            for kind in [EncoderKind::X264, EncoderKind::OpenH264] {
                let mut encoder =
                    Encoder::with_kind_format(width, height, 9, 60, kind, Format::I420).unwrap();
                assert!(encoder.encode_frame(&converted).unwrap().keyframe);
                assert!(!encoder.encode_frame(&converted).unwrap().keyframe);
            }
            let mut fallback = Encoder::for_format(width, height, 10, 60, Format::I420).unwrap();
            unsafe {
                (api.state)(fallback.pipeline, STATE_NULL);
            }
            assert!(fallback.encode_frame(&converted).unwrap().keyframe);
            assert_eq!(fallback.format, Format::I420);
            assert_eq!(fallback.stream, 10);
        }
        assert!(CropConverter::new(
            api,
            CropSpec {
                source_width: 100,
                source_height: 100,
                region: [99, 0, 2, 10]
            }
        )
        .is_err());
    }

    #[test]
    #[ignore = "requires Linux GStreamer; verifies asynchronous wake, bounded leases and resize invalidation"]
    fn async_conversion_drops_backlog_and_invalidates_old_geometry() {
        let api = api().unwrap();
        let wake = Arc::new(FrameWake::new().unwrap());
        let spec = CropSpec {
            source_width: 160,
            source_height: 120,
            region: [7, 5, 128, 96],
        };
        let mut converter = CropConverter::new(api, spec).unwrap();
        converter.wake(wake.clone()).unwrap();
        let mut images = Vec::new();
        for n in 0..100 {
            let image = Arc::new(image(160, 120, n));
            let source = buffer(&image);
            converter.submit(source.0).unwrap();
            images.push(image);
        }
        // At most the currently converting input plus one queued input remain.
        assert!(images.iter().filter(|i| Arc::strong_count(i) > 1).count() <= 2);
        let last = images.last().unwrap();
        let expected = reference(128, 96)
            .convert(buffer(&Arc::new(cropped(last, spec.region))).0)
            .unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        let mut ready = libc::pollfd {
            fd: wake.event_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            if let Some(frame) = converter.pull(0).unwrap() {
                if frame.same_pixels(&expected) {
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "latest frame did not wake the controller"
            );
            unsafe { libc::poll(&mut ready, 1, 100) };
            wake.drain();
        }
        // Leave a converted frame queued under an old window geometry.
        converter.submit(buffer(last).0).unwrap();
        let capture = PipeWireCapture::from_description(
            c"appsrc ! video/x-raw,format=BGRx,width=160,height=120 ! appsink name=frames sync=false",
            wake,
        ).unwrap();
        capture
            .converter_window
            .set(Some(([7, 5, 128, 96], (160, 120), 1.0)));
        *capture.converter.borrow_mut() = Some(converter);
        assert!(capture
            .pull([7, 5, 120, 90], (160, 120), 1.0)
            .unwrap()
            .is_none());
        assert!(capture.converter.borrow().is_none());
        assert!(images.iter().all(|i| Arc::strong_count(i) == 1));
    }

    #[test]
    #[ignore = "opt-in release benchmark: PANEFERRY_CROP_BENCH=1 cargo test --release -p paneferry-agent crop_conversion_benchmark -- --ignored --nocapture"]
    fn crop_conversion_benchmark() {
        if std::env::var("PANEFERRY_CROP_BENCH").as_deref() != Ok("1") {
            return;
        }
        if cfg!(debug_assertions) {
            panic!("run the benchmark with --release");
        }
        let api = api().unwrap();
        for (width, height, source_width, source_height) in [
            (1280, 720, 1920, 1080),
            (2940, 1618, 3840, 2160),
            (4000, 2000, 4096, 2160),
        ] {
            let region = [26, 26, width, height];
            let images: Vec<_> = (0..4)
                .map(|n| Arc::new(image(source_width, source_height, n)))
                .collect();
            let sources: Vec<_> = images.iter().map(buffer).collect();
            for round in 0..4 {
                for direct in if round % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                } {
                    let converter = CropConverter::new(
                        api,
                        CropSpec {
                            source_width,
                            source_height,
                            region,
                        },
                    )
                    .unwrap();
                    let mut encoder = Encoder::for_format(
                        width,
                        height,
                        1,
                        60,
                        if direct { Format::I420 } else { Format::Bgrx },
                    )
                    .unwrap();
                    let mut samples = Vec::new();
                    for n in 0..45 {
                        let start = Instant::now();
                        let mut submit_ms = None;
                        let frame = if direct {
                            converter.submit(sources[n % 4].0).unwrap();
                            submit_ms = Some(start.elapsed().as_secs_f64() * 1000.0);
                            converter.pull(2_000_000_000).unwrap().unwrap()
                        } else {
                            // The previous capture path: map once, copy the selected
                            // rows into an owned BGRx image, then convert in Encoder.
                            let mapping = ReadMap::new(api, sources[n % 4].0).unwrap();
                            let data = copy_bgrx_crop(
                                mapping.bytes().unwrap(),
                                source_width as usize,
                                region.map(|v| v as usize),
                            )
                            .unwrap();
                            Frame::from(CapturedImage {
                                width,
                                height,
                                data,
                                bpp: 32,
                                pad: 32,
                                little: true,
                                masks: (0xff0000, 0xff00, 0xff),
                            })
                        };
                        let prepared = start.elapsed().as_secs_f64() * 1000.0;
                        let packet = encoder.encode_frame(&frame).unwrap();
                        let total = start.elapsed().as_secs_f64() * 1000.0;
                        if n >= 5 {
                            samples.push(serde_json::json!({"prepare_ms":prepared,"submit_ms":submit_ms.unwrap_or(prepared),"encode_ms":total-prepared,"total_ms":total,"bytes":packet.bytes.len()}));
                        }
                    }
                    println!(
                        "CROP_BENCH {}",
                        serde_json::json!({"width":width,"height":height,"round":round,"direct":direct,"samples":samples})
                    );
                }
            }
        }
    }
}
