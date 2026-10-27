//! Optional native WebRTC media runtime. Signaling stays on paired QUIC control.
use anyhow::{bail, Context, Result};
use std::{
    ffi::{c_char, c_int, c_void, CStr, CString},
    path::PathBuf,
    sync::OnceLock,
};
type Object = *mut c_void;
type Callback = unsafe extern "C" fn(Object, c_int, *const c_char, *const u8, usize);
struct Api {
    create: unsafe extern "C" fn(c_int, *const c_char, Callback, Object) -> Object,
    add: unsafe extern "C" fn(Object, u32) -> c_int,
    description: unsafe extern "C" fn(Object, *const c_char, *const c_char) -> c_int,
    candidate: unsafe extern "C" fn(Object, *const c_char, *const c_char) -> c_int,
    send: unsafe extern "C" fn(Object, u32, *const u8, usize, *const u8, usize) -> c_int,
    pair: unsafe extern "C" fn(Object, *mut c_char, usize) -> c_int,
    close: unsafe extern "C" fn(Object),
}
impl Api {
    fn load() -> Result<Self> {
        let path = std::env::var_os("REMOTEAPP_WEBRTC_LIBRARY")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::current_exe()
                    .ok()?
                    .parent()?
                    .parent()
                    .map(|directory| directory.join("lib/lan-remoteapp/libremoteapp_webrtc.so"))
            })
            .context("WebRTC runtime path unavailable")?;
        use std::os::unix::ffi::OsStrExt;
        let path = CString::new(path.as_os_str().as_bytes())?;
        let library = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if library.is_null() {
            bail!("WebRTC runtime unavailable")
        }
        macro_rules! symbol {
            ($name:expr,$signature:ty) => {{
                let address = unsafe { libc::dlsym(library, $name.as_ptr()) };
                if address.is_null() {
                    bail!("WebRTC API unavailable")
                }
                unsafe { std::mem::transmute::<Object, $signature>(address) }
            }};
        }
        // Retain the library for callbacks/global WebRTC worker state for this process.
        Ok(Self {
            create: symbol!(
                c"ra_rtc_create",
                unsafe extern "C" fn(c_int, *const c_char, Callback, Object) -> Object
            ),
            add: symbol!(
                c"ra_rtc_add_video",
                unsafe extern "C" fn(Object, u32) -> c_int
            ),
            description: symbol!(
                c"ra_rtc_description",
                unsafe extern "C" fn(Object, *const c_char, *const c_char) -> c_int
            ),
            candidate: symbol!(
                c"ra_rtc_candidate",
                unsafe extern "C" fn(Object, *const c_char, *const c_char) -> c_int
            ),
            send: symbol!(
                c"ra_rtc_send",
                unsafe extern "C" fn(Object, u32, *const u8, usize, *const u8, usize) -> c_int
            ),
            pair: symbol!(
                c"ra_rtc_selected_pair",
                unsafe extern "C" fn(Object, *mut c_char, usize) -> c_int
            ),
            close: symbol!(c"ra_rtc_close", unsafe extern "C" fn(Object)),
        })
    }
}
fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| Api::load().ok()).as_ref()
}
pub fn available() -> bool {
    api().is_some()
}
pub enum Event {
    Description(String),
    Candidate { candidate: String, mid: String },
    Keyframe(u32),
    State(i32),
    Failed,
}
struct Events(Box<dyn Fn(Event) + Send + Sync>);
unsafe extern "C" fn event(
    context: Object,
    kind: c_int,
    label: *const c_char,
    bytes: *const u8,
    length: usize,
) {
    if context.is_null() || label.is_null() || length > 48 * 1024 || (length > 0 && bytes.is_null())
    {
        return;
    }
    let events = unsafe { &*context.cast::<Events>() };
    let Ok(label) = (unsafe { CStr::from_ptr(label) }).to_str() else {
        return;
    };
    let data = if length == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(bytes, length) }
    };
    let parsed = match kind {
        1 if label == "offer" => std::str::from_utf8(data)
            .ok()
            .map(|s| Event::Description(s.into())),
        2 if length <= 2048 && label.len() <= 32 => {
            std::str::from_utf8(data).ok().map(|s| Event::Candidate {
                candidate: s.into(),
                mid: label.into(),
            })
        }
        4 => label.parse().ok().filter(|id| *id > 0).map(Event::Keyframe),
        5 => label.parse().ok().map(Event::State),
        6 => Some(Event::Failed),
        _ => None,
    };
    if let Some(event) = parsed {
        (events.0)(event);
    }
}
pub struct Peer {
    api: &'static Api,
    handle: Object,
    _events: Box<Events>,
}
impl Peer {
    pub fn new(callback: impl Fn(Event) + Send + Sync + 'static) -> Result<Self> {
        let api = api().context("WebRTC runtime unavailable")?;
        let events = Box::new(Events(Box::new(callback)));
        let context = (&*events as *const Events).cast_mut().cast::<c_void>();
        let bind = std::env::var("REMOTEAPP_WEBRTC_BIND")
            .ok()
            .map(CString::new)
            .transpose()?;
        let handle = unsafe {
            (api.create)(
                1,
                bind.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                event,
                context,
            )
        };
        if handle.is_null() {
            bail!("WebRTC initialization failed")
        }
        Ok(Self {
            api,
            handle,
            _events: events,
        })
    }
    pub fn ready(&self, window: u32) -> Result<bool> {
        let value = unsafe { (self.api.add)(self.handle, window) };
        if value < 0 {
            bail!("WebRTC track rejected")
        }
        Ok(value == 1)
    }
    pub fn answer(&self, sdp: &str) -> Result<()> {
        let sdp = CString::new(sdp)?;
        if unsafe { (self.api.description)(self.handle, sdp.as_ptr(), c"answer".as_ptr()) } < 0 {
            bail!("WebRTC answer rejected")
        }
        Ok(())
    }
    pub fn candidate(&self, candidate: &str, mid: &str) -> Result<()> {
        let candidate = CString::new(candidate)?;
        let mid = CString::new(mid)?;
        if unsafe { (self.api.candidate)(self.handle, candidate.as_ptr(), mid.as_ptr()) } < 0 {
            bail!("WebRTC candidate rejected")
        }
        Ok(())
    }
    pub fn send(&self, window: u32, bytes: &[u8], metadata: &[u8]) -> Result<()> {
        if unsafe {
            (self.api.send)(
                self.handle,
                window,
                bytes.as_ptr(),
                bytes.len(),
                metadata.as_ptr(),
                metadata.len(),
            )
        } < 0
        {
            bail!("WebRTC media send failed")
        }
        Ok(())
    }
    pub fn selected_pair(&self) -> Option<String> {
        let mut bytes = [0u8; 4096];
        let length = unsafe {
            (self.api.pair)(
                self.handle,
                bytes.as_mut_ptr().cast::<c_char>(),
                bytes.len(),
            )
        };
        (length > 0).then(|| String::from_utf8_lossy(&bytes[..length as usize]).into_owned())
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        // The native close drains callbacks before the Events box is dropped.
        unsafe { (self.api.close)(self.handle) };
    }
}
