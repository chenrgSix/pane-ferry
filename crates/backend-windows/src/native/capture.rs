use anyhow::{ensure, Result};
use std::sync::{Arc, Condvar, Mutex};
#[derive(Default)]
pub struct FrameSignal {
    generation: Mutex<u64>,
    arrived: Condvar,
}
impl FrameSignal {
    pub fn generation(&self) -> u64 {
        *self.generation.lock().unwrap()
    }
    pub fn wait(&self, generation: u64, timeout: std::time::Duration) {
        let guard = self.generation.lock().unwrap();
        let _ = self
            .arrived
            .wait_timeout_while(guard, timeout, |value| *value == generation);
    }
    fn notify(&self) {
        let mut generation = self.generation.lock().unwrap();
        *generation = generation.wrapping_add(1);
        self.arrived.notify_one();
    }
}
fn watch(pool: &Direct3D11CaptureFramePool, signal: Arc<FrameSignal>) -> Result<i64> {
    Ok(
        pool.FrameArrived(&windows::Foundation::TypedEventHandler::new(move |_, _| {
            signal.notify();
            Ok(())
        }))?,
    )
}
use windows::{
    core::Interface,
    Graphics::{
        Capture::*,
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::HWND,
        Graphics::{Direct3D::*, Direct3D11::*, Dxgi::IDXGIDevice},
        System::WinRT::{Direct3D11::*, Graphics::Capture::IGraphicsCaptureItemInterop},
    },
};

pub struct Capture {
    item: GraphicsCaptureItem,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    runtime_device: IDirect3DDevice,
    size: windows::Graphics::SizeInt32,
    staging: Option<ID3D11Texture2D>,
    signal: Arc<FrameSignal>,
    frame_event: i64,
}
impl Capture {
    pub fn new(hwnd: HWND, signal: Arc<FrameSignal>) -> Result<Self> {
        ensure!(
            GraphicsCaptureSession::IsSupported()?,
            "capture_unavailable"
        );
        unsafe {
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                windows::Win32::Foundation::HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.unwrap();
            let context = context.unwrap();
            let runtime_device: IDirect3DDevice =
                CreateDirect3D11DeviceFromDXGIDevice(&device.cast::<IDXGIDevice>()?)?.cast()?;
            let interop: IGraphicsCaptureItemInterop =
                windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
            let item: GraphicsCaptureItem = interop.CreateForWindow(hwnd)?;
            let size = item.Size()?;
            paneferry_protocol::dimensions(size.Width as u32, size.Height as u32)?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                &runtime_device,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                size,
            )?;
            let frame_event = watch(&pool, signal.clone())?;
            let session = pool.CreateCaptureSession(&item)?;
            session.SetIsCursorCaptureEnabled(false)?;
            // Optional on newer Windows. Avoid a second OS-side cadence gate;
            // the publisher owns the negotiated frame rate and drains stale raw frames.
            let _ =
                session.SetMinUpdateInterval(windows::Foundation::TimeSpan { Duration: 40_000 });
            session.StartCapture()?;
            Ok(Self {
                item,
                pool,
                session,
                device,
                context,
                runtime_device,
                size,
                staging: None,
                signal,
                frame_event,
            })
        }
    }
    pub fn frame_into(&mut self, pixels: &mut Vec<u8>) -> Result<Option<(u32, u32)>> {
        let mut frame = match self.pool.TryGetNextFrame() {
            Ok(frame) => frame,
            // WGC returns S_OK + a null frame while its queue is empty. The
            // windows-rs non-null projection represents that as Error::empty()
            // (HRESULT S_OK), not E_POINTER. It is normal before the first frame.
            Err(e)
                if e.code() == windows::Win32::Foundation::S_OK
                    || e.code() == windows::Win32::Foundation::E_POINTER =>
            {
                return Ok(None)
            }
            Err(e) => return Err(e.into()),
        };
        // The two-slot WGC pool can contain an older frame after encoding or
        // network backpressure. Discard raw superseded frames before GPU readback.
        if let Ok(newest) = self.pool.TryGetNextFrame() {
            frame.Close()?;
            frame = newest;
        }
        let _timing = crate::metrics::span("capture.readback_new");
        let size = frame.ContentSize()?;
        if size.Width <= 0 || size.Height <= 0 {
            frame.Close()?;
            return Ok(None);
        }
        paneferry_protocol::dimensions(size.Width as u32, size.Height as u32)?;
        if size != self.size {
            frame.Close()?;
            drop(frame);
            // Recreate alone can leave WGC silent after fullscreen/restore,
            // especially for a static surface. Start a fresh capture session on
            // the same validated item/device so the new size gets an initial frame.
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                &self.runtime_device,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                size,
            )?;
            let frame_event = watch(&pool, self.signal.clone())?;
            let session = pool.CreateCaptureSession(&self.item)?;
            session.SetIsCursorCaptureEnabled(false)?;
            // Optional on newer Windows. Avoid a second OS-side cadence gate;
            // the publisher owns the negotiated frame rate and drains stale raw frames.
            let _ =
                session.SetMinUpdateInterval(windows::Foundation::TimeSpan { Duration: 40_000 });
            session.StartCapture()?;
            self.session.Close()?;
            self.pool.RemoveFrameArrived(self.frame_event)?;
            self.pool.Close()?;
            self.session = session;
            self.pool = pool;
            self.frame_event = frame_event;
            self.size = size;
            self.staging = None;
            return Ok(None);
        }
        unsafe {
            let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
            let texture: ID3D11Texture2D = access.GetInterface()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            ensure!(
                desc.Width >= size.Width as u32 && desc.Height >= size.Height as u32,
                "invalid capture size"
            );
            if let Some(staging) = &self.staging {
                let mut cached = D3D11_TEXTURE2D_DESC::default();
                staging.GetDesc(&mut cached);
                // WGC texture allocation can lag ContentSize during a resize.
                // CopyResource requires matching allocations, not just visible size.
                if cached.Width != desc.Width
                    || cached.Height != desc.Height
                    || cached.Format != desc.Format
                {
                    self.staging = None;
                }
            }
            if self.staging.is_none() {
                desc.Usage = D3D11_USAGE_STAGING;
                desc.BindFlags = 0;
                desc.MiscFlags = 0;
                desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                self.device
                    .CreateTexture2D(&desc, None, Some(&mut self.staging))?;
            }
            let staging = self.staging.as_ref().unwrap();
            self.context.CopyResource(staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let row = size.Width as usize * 4;
            let result = if mapped.RowPitch as usize >= row {
                pixels.resize(row * size.Height as usize, 0);
                for y in 0..size.Height as usize {
                    let source = std::slice::from_raw_parts(
                        mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),
                        row,
                    );
                    pixels[y * row..(y + 1) * row].copy_from_slice(source);
                }
                Ok(Some((size.Width as u32, size.Height as u32)))
            } else {
                Err(anyhow::anyhow!("invalid capture pitch"))
            };
            self.context.Unmap(staging, 0);
            frame.Close()?;
            result
        }
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.RemoveFrameArrived(self.frame_event);
        let _ = self.pool.Close();
    }
}
