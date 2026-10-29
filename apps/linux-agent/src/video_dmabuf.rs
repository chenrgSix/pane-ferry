//! Optional public GStreamer allocator API. The final GstMemory reference owns
//! the capture lease, including when a plugin retains memory after push/pull.
use super::*;
use paneferry_backend_x11::DmaBufFrame;
use std::os::fd::IntoRawFd;

struct DmaApi {
    allocator: unsafe extern "C" fn() -> Object,
    alloc: unsafe extern "C" fn(Object, c_int, usize) -> Object,
    buffer: unsafe extern "C" fn() -> Object,
    append: unsafe extern "C" fn(Object, Object),
    video_meta: unsafe extern "C" fn(
        Object,
        c_uint,
        c_int,
        c_uint,
        c_uint,
        c_uint,
        *const usize,
        *const c_int,
    ) -> Object,
    video_format: unsafe extern "C" fn(*const c_char) -> c_int,
    quark: unsafe extern "C" fn(*const c_char) -> u32,
    qdata: unsafe extern "C" fn(Object, u32, Object, Option<Destroy>),
}
fn dma_api() -> Option<&'static DmaApi> {
    static API: OnceLock<Option<DmaApi>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        macro_rules! lib {
            ($s:expr) => {{
                let l = libc::dlopen($s.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
                if l.is_null() {
                    return None;
                }
                l
            }};
        }
        let a = lib!(c"libgstallocators-1.0.so.0");
        let c = lib!(c"libgstreamer-1.0.so.0");
        let v = lib!(c"libgstvideo-1.0.so.0");
        let g = lib!(c"libglib-2.0.so.0");
        macro_rules! symbol {
            ($l:ident,$s:expr,$t:ty) => {{
                let p = libc::dlsym($l, $s.as_ptr());
                if p.is_null() {
                    return None;
                }
                std::mem::transmute::<Object, $t>(p)
            }};
        }
        Some(DmaApi {
            allocator: symbol!(
                a,
                c"gst_dmabuf_allocator_new",
                unsafe extern "C" fn() -> Object
            ),
            alloc: symbol!(
                a,
                c"gst_dmabuf_allocator_alloc",
                unsafe extern "C" fn(Object, c_int, usize) -> Object
            ),
            buffer: symbol!(c, c"gst_buffer_new", unsafe extern "C" fn() -> Object),
            append: symbol!(
                c,
                c"gst_buffer_append_memory",
                unsafe extern "C" fn(Object, Object)
            ),
            video_meta: symbol!(
                v,
                c"gst_buffer_add_video_meta_full",
                unsafe extern "C" fn(
                    Object,
                    c_uint,
                    c_int,
                    c_uint,
                    c_uint,
                    c_uint,
                    *const usize,
                    *const c_int,
                ) -> Object
            ),
            video_format: symbol!(
                v,
                c"gst_video_format_from_string",
                unsafe extern "C" fn(*const c_char) -> c_int
            ),
            quark: symbol!(
                g,
                c"g_quark_from_static_string",
                unsafe extern "C" fn(*const c_char) -> u32
            ),
            qdata: symbol!(
                c,
                c"gst_mini_object_set_qdata",
                unsafe extern "C" fn(Object, u32, Object, Option<Destroy>)
            ),
        })
    })
    .as_ref()
}
pub(super) fn available() -> bool {
    dma_api().is_some()
}
unsafe extern "C" fn release(pointer: Object) {
    drop(unsafe { Box::from_raw(pointer.cast::<Arc<DmaBufFrame>>()) });
}
pub(super) fn input(frame: &Arc<DmaBufFrame>, api: &'static Api) -> Result<Object> {
    let dma = dma_api().context("DMA-buf allocator unavailable")?;
    let owned = frame.fd().try_clone_to_owned()?;
    unsafe {
        let allocator = (dma.allocator)();
        anyhow::ensure!(!allocator.is_null(), "DMA-buf allocator failed");
        let fd = owned.into_raw_fd();
        let memory = (dma.alloc)(allocator, fd, frame.length());
        (api.unref)(allocator);
        if memory.is_null() {
            libc::close(fd);
            bail!("DMA-buf allocation failed");
        }
        let lease = Box::into_raw(Box::new(frame.clone())).cast();
        (dma.qdata)(
            memory,
            (dma.quark)(c"paneferry-private-gpu-lease".as_ptr()),
            lease,
            Some(release),
        );
        let buffer = (dma.buffer)();
        if buffer.is_null() {
            (api.mini_unref)(memory);
            bail!("DMA-buf wrapping failed");
        }
        (dma.append)(buffer, memory);
        let offsets = [0usize; 4];
        let strides = [frame.stride() as c_int, 0, 0, 0];
        if (dma.video_meta)(
            buffer,
            0,
            (dma.video_format)(c"BGRx".as_ptr()),
            frame.width() + frame.width() % 2,
            frame.height() + frame.height() % 2,
            1,
            offsets.as_ptr(),
            strides.as_ptr(),
        )
        .is_null()
        {
            (api.mini_unref)(buffer);
            bail!("DMA-buf video metadata failed");
        }
        Ok(buffer)
    }
}
