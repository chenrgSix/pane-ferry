//! Optional D3D11 composition and color conversion on a shared capture device.
//! Only the final encoder input is read back; no application pixels leave the scene.
use crate::color::ColorMode;
use anyhow::{ensure, Context, Result};
use std::{
    mem::size_of,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::{
    core::{s, Interface, PCSTR},
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::{Fxc::*, *},
            Direct3D11::*,
            Dxgi::{Common::*, IDXGIDevice},
        },
        System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice,
    },
};

pub struct Device {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub runtime: IDirect3DDevice,
}
impl Device {
    pub fn new() -> Result<Self> {
        unsafe {
            let mut device = None;
            let mut context = None;
            let result = D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            );
            if result.is_err() {
                // Capture/software encoding must not acquire a video-device
                // requirement merely because direct hardware encoding exists.
                device = None;
                context = None;
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )?;
            }
            let device = device.unwrap();
            let context = context.unwrap();
            // Hardware MFTs use the same immediate context on their own threads.
            let _ = context
                .cast::<ID3D11Multithread>()?
                .SetMultithreadProtected(true);
            let runtime =
                CreateDirect3D11DeviceFromDXGIDevice(&device.cast::<IDXGIDevice>()?)?.cast()?;
            Ok(Self {
                device,
                context,
                runtime,
            })
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Params {
    width: u32,
    height: u32,
    ew: u32,
    eh: u32,
    canvas: [i32; 4],
    surface: [i32; 4],
    region: [u32; 4],
    full: u32,
    padding: [u32; 3],
}
pub struct Layer {
    pub texture: ID3D11Texture2D,
    view: ID3D11ShaderResourceView,
    pub width: u32,
    pub height: u32,
}
impl Layer {
    pub fn new(device: &Device, texture: ID3D11Texture2D, width: u32, height: u32) -> Result<Self> {
        let mut view = None;
        unsafe {
            device
                .device
                .CreateShaderResourceView(&texture, None, Some(&mut view))?;
        }
        Ok(Self {
            texture,
            view: view.unwrap(),
            width,
            height,
        })
    }
}
struct Canvas {
    width: u32,
    height: u32,
    view: ID3D11ShaderResourceView,
    target: ID3D11UnorderedAccessView,
}
struct Readback {
    size: usize,
    output: ID3D11Buffer,
    target: ID3D11UnorderedAccessView,
    staging: ID3D11Buffer,
}
struct TextureSlot {
    texture: ID3D11Texture2D,
    y: ID3D11UnorderedAccessView,
    uv: ID3D11UnorderedAccessView,
    busy: Arc<AtomicBool>,
}
/// The slot stays unavailable until the MFT releases its tracked input sample.
pub struct TextureFrame {
    pub texture: ID3D11Texture2D,
    busy: Arc<AtomicBool>,
}
impl Drop for TextureFrame {
    fn drop(&mut self) {
        self.busy.store(false, Ordering::Release);
    }
}
#[cfg(test)]
impl TextureFrame {
    fn read_nv12(&self, device: &Device) -> Result<Vec<u8>> {
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            self.texture.GetDesc(&mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            device
                .device
                .CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging = staging.unwrap();
            device.context.CopyResource(&staging, &self.texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            device
                .context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let mut data = vec![0; (desc.Width * desc.Height * 3 / 2) as usize];
            for (y, row) in data.chunks_exact_mut(desc.Width as usize).enumerate() {
                row.copy_from_slice(std::slice::from_raw_parts(
                    mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),
                    row.len(),
                ));
            }
            device.context.Unmap(&staging, 0);
            Ok(data)
        }
    }
}
pub struct Compositor {
    pub device: Device,
    compose: ID3D11ComputeShader,
    convert: ID3D11ComputeShader,
    convert_texture: ID3D11ComputeShader,
    textures: Vec<TextureSlot>,
    texture_size: (u32, u32),
    params: ID3D11Buffer,
    canvas: Option<Canvas>,
    readback: Option<Readback>,
    current: Params,
}
fn shaders() -> Result<&'static (Vec<u8>, Vec<u8>, Vec<u8>)> {
    static SHADERS: OnceLock<std::result::Result<(Vec<u8>, Vec<u8>, Vec<u8>), String>> =
        OnceLock::new();
    SHADERS
        .get_or_init(|| {
            fn compile(entry: PCSTR) -> Result<Vec<u8>> {
                let source = include_bytes!("gpu.hlsl");
                let mut code = None;
                let mut errors = None;
                unsafe {
                    if let Err(error) = D3DCompile(
                        source.as_ptr().cast(),
                        source.len(),
                        s!("paneferry-gpu"),
                        None,
                        None,
                        entry,
                        s!("cs_5_0"),
                        D3DCOMPILE_OPTIMIZATION_LEVEL3,
                        0,
                        &mut code,
                        Some(&mut errors),
                    ) {
                        let detail = errors
                            .map(|blob| {
                                String::from_utf8_lossy(std::slice::from_raw_parts(
                                    blob.GetBufferPointer().cast(),
                                    blob.GetBufferSize(),
                                ))
                                .into_owned()
                            })
                            .unwrap_or_default();
                        anyhow::bail!("compile shader: {error}: {detail}");
                    }
                    let code = code.unwrap();
                    Ok(std::slice::from_raw_parts(
                        code.GetBufferPointer().cast(),
                        code.GetBufferSize(),
                    )
                    .to_vec())
                }
            }
            (|| {
                Ok((
                    compile(s!("compose"))?,
                    compile(s!("convert"))?,
                    compile(s!("convertTexture"))?,
                ))
            })()
            .map_err(|e: anyhow::Error| format!("{e:#}"))
        })
        .as_ref()
        .map_err(|e| anyhow::anyhow!(e.clone()))
}
impl Compositor {
    #[cfg(test)]
    pub fn read_bgra(&self) -> Result<Vec<u8>> {
        let canvas = self.canvas.as_ref().context("missing canvas")?;
        unsafe {
            let resource = canvas.view.GetResource()?;
            let texture: ID3D11Texture2D = resource.cast()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            self.device
                .device
                .CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging = staging.unwrap();
            let ctx = &self.device.context;
            ctx.CopyResource(&staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let row = canvas.width as usize * 4;
            let mut bytes = vec![0; row * canvas.height as usize];
            for y in 0..canvas.height as usize {
                bytes[y * row..][..row].copy_from_slice(std::slice::from_raw_parts(
                    mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),
                    row,
                ));
            }
            ctx.Unmap(&staging, 0);
            Ok(bytes)
        }
    }
    pub fn new() -> Result<Self> {
        let device = Device::new()?;
        ensure!(
            unsafe { device.device.GetFeatureLevel().0 } >= D3D_FEATURE_LEVEL_11_0.0,
            "GPU feature level unavailable"
        );
        let (cs, convert, texture) = shaders()?;
        let mut compose_shader = None;
        let mut convert_shader = None;
        let mut texture_shader = None;
        let mut params = None;
        unsafe {
            device
                .device
                .CreateComputeShader(cs, None, Some(&mut compose_shader))?;
            device
                .device
                .CreateComputeShader(convert, None, Some(&mut convert_shader))?;
            device
                .device
                .CreateComputeShader(texture, None, Some(&mut texture_shader))?;
            device.device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: size_of::<Params>() as u32,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut params),
            )?;
        }
        Ok(Self {
            device,
            compose: compose_shader.unwrap(),
            convert: convert_shader.unwrap(),
            convert_texture: texture_shader.unwrap(),
            textures: Vec::new(),
            texture_size: (0, 0),
            params: params.unwrap(),
            canvas: None,
            readback: None,
            current: Params::default(),
        })
    }
    fn update(&self) {
        unsafe {
            self.device.context.UpdateSubresource(
                &self.params,
                0,
                None,
                (&self.current as *const Params).cast(),
                0,
                0,
            );
            self.device
                .context
                .CSSetConstantBuffers(0, Some(&[Some(self.params.clone())]));
        }
    }
    pub fn begin(
        &mut self,
        width: u32,
        height: u32,
        rect: windows::Win32::Foundation::RECT,
    ) -> Result<()> {
        paneferry_protocol::dimensions(width, height)?;
        ensure!(
            rect.right as i64 - rect.left as i64 > 0
                && rect.bottom as i64 - rect.top as i64 > 0
                && rect.right as i64 - rect.left as i64 <= 500_000
                && rect.bottom as i64 - rect.top as i64 <= 500_000,
            "GPU canvas coordinates unavailable"
        );
        unsafe {
            if self
                .canvas
                .as_ref()
                .is_none_or(|c| c.width != width || c.height != height)
            {
                let mut texture = None;
                let mut view = None;
                let mut target = None;
                self.device.device.CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Width: width,
                        Height: height,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: DXGI_FORMAT_R32_UINT,
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        Usage: D3D11_USAGE_DEFAULT,
                        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_UNORDERED_ACCESS.0)
                            as u32,
                        ..Default::default()
                    },
                    None,
                    Some(&mut texture),
                )?;
                let texture = texture.unwrap();
                self.device
                    .device
                    .CreateShaderResourceView(&texture, None, Some(&mut view))?;
                self.device
                    .device
                    .CreateUnorderedAccessView(&texture, None, Some(&mut target))?;
                self.canvas = Some(Canvas {
                    width,
                    height,
                    view: view.unwrap(),
                    target: target.unwrap(),
                });
            }
            self.current = Params {
                width,
                height,
                canvas: [
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                ],
                ..Default::default()
            };
            self.device.context.ClearUnorderedAccessViewUint(
                &self.canvas.as_ref().unwrap().target,
                &[0xff181818; 4],
            );
        }
        Ok(())
    }
    pub fn paint(&mut self, layer: &Layer, rect: windows::Win32::Foundation::RECT) -> Result<()> {
        let p = &mut self.current;
        let [cx, cy, cw, ch] = p.canvas;
        ensure!(
            rect.right - rect.left == layer.width as i32
                && rect.bottom - rect.top == layer.height as i32,
            "GPU layer size mismatch"
        );
        let left = ((rect.left - cx) as i64 * p.width as i64 / cw as i64).max(0) as u32;
        let top = ((rect.top - cy) as i64 * p.height as i64 / ch as i64).max(0) as u32;
        let right = (((rect.right - cx) as i64 * p.width as i64 + cw as i64 - 1) / cw as i64)
            .min(p.width as i64) as u32;
        let bottom = (((rect.bottom - cy) as i64 * p.height as i64 + ch as i64 - 1) / ch as i64)
            .min(p.height as i64) as u32;
        if right <= left || bottom <= top {
            return Ok(());
        }
        p.surface = [rect.left, rect.top, layer.width as i32, layer.height as i32];
        p.region = [left, top, right, bottom];
        self.update();
        unsafe {
            let ctx = &self.device.context;
            ctx.CSSetShader(&self.compose, None);
            ctx.CSSetShaderResources(0, Some(&[Some(layer.view.clone())]));
            ctx.CSSetUnorderedAccessViews(
                0,
                1,
                Some([Some(self.canvas.as_ref().unwrap().target.clone())].as_ptr()),
                None,
            );
            ctx.Dispatch((right - left).div_ceil(16), (bottom - top).div_ceil(16), 1);
            ctx.CSSetShaderResources(0, Some(&[None]));
            ctx.CSSetUnorderedAccessViews(0, 1, Some([None].as_ptr()), None);
        }
        Ok(())
    }
    pub fn convert_texture(&mut self, mode: ColorMode) -> Result<TextureFrame> {
        let _timing = crate::metrics::span("gpu.convert_texture_submit");
        let (ew, eh) = mode.dimensions(self.current.width, self.current.height)?;
        if self.texture_size != (ew, eh) {
            self.textures.clear(); // Outstanding frames retain their own resources.
            self.texture_size = (ew, eh);
        }
        let index = if let Some(index) = self
            .textures
            .iter()
            .position(|s| !s.busy.load(Ordering::Acquire))
        {
            index
        } else {
            ensure!(
                self.textures.len() < 3,
                "hardware input texture pool exhausted"
            );
            unsafe {
                let mut texture = None;
                self.device.device.CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Width: ew,
                        Height: eh,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: DXGI_FORMAT_NV12,
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        Usage: D3D11_USAGE_DEFAULT,
                        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_UNORDERED_ACCESS.0)
                            as u32,
                        ..Default::default()
                    },
                    None,
                    Some(&mut texture),
                )?;
                let texture = texture.unwrap();
                let view = |format| -> Result<ID3D11UnorderedAccessView> {
                    let mut view = None;
                    self.device.device.CreateUnorderedAccessView(
                        &texture,
                        Some(&D3D11_UNORDERED_ACCESS_VIEW_DESC {
                            Format: format,
                            ViewDimension: D3D11_UAV_DIMENSION_TEXTURE2D,
                            Anonymous: D3D11_UNORDERED_ACCESS_VIEW_DESC_0 {
                                Texture2D: D3D11_TEX2D_UAV { MipSlice: 0 },
                            },
                        }),
                        Some(&mut view),
                    )?;
                    Ok(view.unwrap())
                };
                let y = view(DXGI_FORMAT_R8_UINT)?;
                let uv = view(DXGI_FORMAT_R8G8_UINT)?;
                self.textures.push(TextureSlot {
                    texture,
                    y,
                    uv,
                    busy: Arc::new(AtomicBool::new(false)),
                });
            }
            self.textures.len() - 1
        };
        self.current.ew = ew;
        self.current.eh = eh;
        self.current.full = u32::from(mode.is_full());
        self.update();
        let slot = &self.textures[index];
        slot.busy.store(true, Ordering::Release);
        let frame = TextureFrame {
            texture: slot.texture.clone(),
            busy: slot.busy.clone(),
        };
        unsafe {
            let ctx = &self.device.context;
            ctx.CSSetShader(&self.convert_texture, None);
            ctx.CSSetShaderResources(
                1,
                Some(&[Some(
                    self.canvas
                        .as_ref()
                        .context("GPU canvas unavailable")?
                        .view
                        .clone(),
                )]),
            );
            ctx.CSSetUnorderedAccessViews(
                2,
                2,
                Some([Some(slot.y.clone()), Some(slot.uv.clone())].as_ptr()),
                None,
            );
            ctx.Dispatch(ew.div_ceil(16), eh.div_ceil(16), 1);
            ctx.CSSetShaderResources(1, Some(&[None]));
            ctx.CSSetUnorderedAccessViews(2, 2, Some([None, None].as_ptr()), None);
            // Submit writes before the hardware encoder consumes this surface.
            ctx.Flush();
            self.device.device.GetDeviceRemovedReason()?;
        }
        Ok(frame)
    }
    pub fn convert(&mut self, mode: ColorMode, output: &mut Vec<u8>) -> Result<()> {
        let _timing = crate::metrics::span("gpu.convert_readback");
        let (ew, eh) = mode.dimensions(self.current.width, self.current.height)?;
        let length = ew as usize * eh as usize * 3 / 2;
        let size = length.div_ceil(4) * 4;
        unsafe {
            if self.readback.as_ref().is_none_or(|b| b.size != size) {
                let mut buffer = None;
                let mut target = None;
                let mut staging = None;
                let mut desc = D3D11_BUFFER_DESC {
                    ByteWidth: size as u32,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_UNORDERED_ACCESS.0 as u32,
                    MiscFlags: D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS.0 as u32,
                    ..Default::default()
                };
                self.device
                    .device
                    .CreateBuffer(&desc, None, Some(&mut buffer))?;
                let buffer = buffer.unwrap();
                self.device.device.CreateUnorderedAccessView(
                    &buffer,
                    Some(&D3D11_UNORDERED_ACCESS_VIEW_DESC {
                        Format: DXGI_FORMAT_R32_TYPELESS,
                        ViewDimension: D3D11_UAV_DIMENSION_BUFFER,
                        Anonymous: D3D11_UNORDERED_ACCESS_VIEW_DESC_0 {
                            Buffer: D3D11_BUFFER_UAV {
                                FirstElement: 0,
                                NumElements: (size / 4) as u32,
                                Flags: D3D11_BUFFER_UAV_FLAG_RAW.0 as u32,
                            },
                        },
                    }),
                    Some(&mut target),
                )?;
                desc.Usage = D3D11_USAGE_STAGING;
                desc.BindFlags = 0;
                desc.MiscFlags = 0;
                desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                self.device
                    .device
                    .CreateBuffer(&desc, None, Some(&mut staging))?;
                self.readback = Some(Readback {
                    size,
                    output: buffer,
                    target: target.unwrap(),
                    staging: staging.unwrap(),
                });
            }
            self.current.ew = ew;
            self.current.eh = eh;
            self.current.full = u32::from(mode.is_full());
            self.update();
            let readback = self.readback.as_ref().unwrap();
            let ctx = &self.device.context;
            ctx.CSSetShader(&self.convert, None);
            ctx.CSSetShaderResources(
                1,
                Some(&[Some(
                    self.canvas
                        .as_ref()
                        .context("GPU canvas unavailable")?
                        .view
                        .clone(),
                )]),
            );
            ctx.CSSetUnorderedAccessViews(
                1,
                1,
                Some([Some(readback.target.clone())].as_ptr()),
                None,
            );
            ctx.Dispatch((size as u32 / 4).div_ceil(256), 1, 1);
            ctx.CSSetShaderResources(1, Some(&[None]));
            ctx.CSSetUnorderedAccessViews(1, 1, Some([None].as_ptr()), None);
            ctx.CopyResource(&readback.staging, &readback.output);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&readback.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            output.resize(length, 0);
            output.copy_from_slice(std::slice::from_raw_parts(mapped.pData.cast(), length));
            ctx.Unmap(&readback.staging, 0);
            self.device.device.GetDeviceRemovedReason()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::RECT;

    #[test]
    #[ignore = "uses the local D3D11 device, synthetic pixels only"]
    fn gpu_composition_and_both_color_modes_match_cpu() -> Result<()> {
        let mut gpu = Compositor::new()?;
        for (w, h) in [(3u32, 5u32), (64, 32), (1602, 932), (2051, 65), (33, 2049)] {
            let rect = RECT {
                left: -30,
                top: -20,
                right: w as i32 - 30,
                bottom: h as i32 - 20,
            };
            let mut pixels = Vec::new();
            let mut expected = vec![0; (w * h * 4) as usize];
            for i in 0..w * h {
                let a = [0u8, 64, 128, 255][i as usize % 4];
                pixels.extend_from_slice(&[a / 4, a / 2, a, a]);
            }
            super::super::blend::over(&pixels, &mut expected, true);
            let mut texture = None;
            unsafe {
                gpu.device.device.CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Width: w,
                        Height: h,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        Usage: D3D11_USAGE_DEFAULT,
                        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                        ..Default::default()
                    },
                    Some(&D3D11_SUBRESOURCE_DATA {
                        pSysMem: pixels.as_ptr().cast(),
                        SysMemPitch: w * 4,
                        SysMemSlicePitch: 0,
                    }),
                    Some(&mut texture),
                )?;
            }
            let layer = Layer::new(&gpu.device, texture.unwrap(), w, h)?;
            gpu.begin(w, h, rect)?;
            gpu.paint(&layer, rect)?;
            // Overlay the same semi-transparent surface again; verifies UAV
            // ordering and blending against the already composed background.
            gpu.paint(&layer, rect)?;
            super::super::blend::over(&pixels, &mut expected, false);
            for mode in [
                ColorMode::Compatible,
                ColorMode::Full,
                ColorMode::FullAligned,
            ] {
                let mut actual = Vec::new();
                let mut reference = Vec::new();
                gpu.convert(mode, &mut actual)?;
                let direct = gpu.convert_texture(mode)?;
                assert_eq!(
                    direct.read_nv12(&gpu.device)?,
                    actual,
                    "texture planes must match packed NV12"
                );
                let second = gpu.convert_texture(mode)?;
                let third = gpu.convert_texture(mode)?;
                assert!(
                    gpu.convert_texture(mode).is_err(),
                    "never overwrite a retained encoder surface"
                );
                drop((direct, second, third));
                match mode {
                    ColorMode::Compatible => {
                        crate::pixels::nv12_into(&expected, w, h, &mut reference)?
                    }
                    ColorMode::Full | ColorMode::FullAligned => {
                        crate::color::atlas_into_mode(mode, &expected, w, h, &mut reference)?
                    }
                }
                assert_eq!(actual.len(), reference.len());
                if let Some(i) = actual.iter().zip(&reference).position(|(a, b)| a != b) {
                    anyhow::bail!(
                        "GPU mismatch {w}x{h} {mode:?} offset={i} actual={} expected={}",
                        actual[i],
                        reference[i]
                    );
                }
            }
        }
        Ok(())
    }
}
