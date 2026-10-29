use anyhow::{ensure, Result};
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
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    runtime_device: IDirect3DDevice,
    size: windows::Graphics::SizeInt32,
    staging: Option<ID3D11Texture2D>,
}
impl Capture {
    pub fn new(hwnd: HWND) -> Result<Self> {
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
            let session = pool.CreateCaptureSession(&item)?;
            session.SetIsCursorCaptureEnabled(false)?;
            session.StartCapture()?;
            Ok(Self {
                pool,
                session,
                device,
                context,
                runtime_device,
                size,
                staging: None,
            })
        }
    }
    pub fn frame(&mut self) -> Result<Option<(u32, u32, Vec<u8>)>> {
        let frame = match self.pool.TryGetNextFrame() {
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
        let size = frame.ContentSize()?;
        if size.Width <= 0 || size.Height <= 0 {
            frame.Close()?;
            return Ok(None);
        }
        paneferry_protocol::dimensions(size.Width as u32, size.Height as u32)?;
        if size != self.size {
            frame.Close()?;
            self.pool.Recreate(
                &self.runtime_device,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                size,
            )?;
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
                let mut pixels = vec![0; row * size.Height as usize];
                for y in 0..size.Height as usize {
                    let source = std::slice::from_raw_parts(
                        mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),
                        row,
                    );
                    pixels[y * row..(y + 1) * row].copy_from_slice(source);
                }
                Ok(Some((size.Width as u32, size.Height as u32, pixels)))
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
        let _ = self.pool.Close();
    }
}
