import Foundation
import CoreVideo
import Metal

enum VideoColor: String {
    case compatible = "420", full = "444-atlas-v1"
    func dimensions(width: Int, height: Int) throws -> (Int, Int) {
        guard width > 0, height > 0, width <= 4096, height <= 4096,
              width * height <= 8_000_000 else { throw VideoDecodeError.invalidFrame }
        let w = width + width % 2, h = height + height % 2
        if self == .compatible { return (w, h) }
        let ew = min(w * 2, 4096), rows = (2 * w * h + ew - 1) / ew
        let eh = rows + rows % 2
        guard eh <= 4096, ew * eh <= 16_000_000 else { throw VideoDecodeError.invalidFrame }
        return (ew, eh)
    }
}

/// Recover full-resolution Y, U and V from a negotiated NV12 carrier. The
/// carrier is ordinary H.264 4:2:0; its samples are data, not display colors.
/// This preserves chroma sampling, not lossless compression.
final class ChromaAtlas {
    private struct GPU {
        let device: MTLDevice, queue: MTLCommandQueue, pipeline: MTLComputePipelineState
    }
    private static let gpu: GPU? = {
        guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else { return nil }
        let source = """
        #include <metal_stdlib>
        using namespace metal;
        kernel void unpack(texture2d<float, access::read> yplane [[texture(0)]],
                           texture2d<float, access::read> uvplane [[texture(1)]],
                           texture2d<float, access::write> output [[texture(2)]],
                           uint2 p [[thread_position_in_grid]]) {
            uint width=output.get_width(), height=output.get_height();
            if(p.x>=width || p.y>=height) return;
            uint w=width+width%2, n=w*(height+height%2), i=p.y*w+p.x;
            uint ew=yplane.get_width();
            float y=yplane.read(uint2(i%ew,i/ew)).r*255.0;
            uint ui=n+i;
            float u=yplane.read(uint2(ui%ew,ui/ew)).r*255.0;
            float2 packed=uvplane.read(uint2((i%ew)/2,i/ew)).rg;
            float v=(i%2==0 ? packed.r : packed.g)*255.0;
            float c=y-16.0, d=u-128.0, e=v-128.0;
            float3 rgb=float3(298.0*c+409.0*e,298.0*c-100.0*d-208.0*e,298.0*c+516.0*d)/256.0/255.0;
            output.write(float4(clamp(rgb,0.0,1.0),1.0),p);
        }
        """
        guard let library = try? device.makeLibrary(source: source, options: nil),
              let function = library.makeFunction(name: "unpack"),
              let pipeline = try? device.makeComputePipelineState(function: function) else { return nil }
        return GPU(device: device, queue: queue, pipeline: pipeline)
    }()
    private var cache: CVMetalTextureCache?
    private var pool: CVPixelBufferPool?
    private var poolWidth = 0, poolHeight = 0
    private var gpuFailed = false

    func unpack(_ input: CVPixelBuffer, width: Int, height: Int) throws -> CVPixelBuffer {
        let (ew, eh) = try VideoColor.full.dimensions(width: width, height: height)
        guard CVPixelBufferGetPixelFormatType(input) == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
              CVPixelBufferGetPlaneCount(input) == 2, CVPixelBufferGetWidth(input) == ew,
              CVPixelBufferGetHeight(input) == eh else { throw VideoDecodeError.invalidFrame }
        if pool == nil || poolWidth != width || poolHeight != height {
            let attrs: [CFString: Any] = [kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_32BGRA,
                kCVPixelBufferWidthKey: width, kCVPixelBufferHeightKey: height,
                kCVPixelBufferMetalCompatibilityKey: true, kCVPixelBufferIOSurfacePropertiesKey: [:]]
            var next: CVPixelBufferPool?
            let status = CVPixelBufferPoolCreate(kCFAllocatorDefault, nil, attrs as CFDictionary, &next)
            guard status == kCVReturnSuccess, let next = next else { throw VideoDecodeError.decoder(status) }
            pool = next; poolWidth = width; poolHeight = height
        }
        var buffer: CVPixelBuffer?
        let status = CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, pool!, &buffer)
        guard status == kCVReturnSuccess, let buffer = buffer else { throw VideoDecodeError.decoder(status) }
        if !gpuFailed, let gpu = Self.gpu {
            do {
                try render(input, into: buffer, gpu: gpu)
                return buffer
            } catch {
                gpuFailed = true
                fputs("PF_COLOR unpack=cpu fallback_from=metal\n", stderr)
            }
        }
        try software(input, into: buffer)
        return buffer
    }
    private func render(_ input: CVPixelBuffer, into output: CVPixelBuffer, gpu: GPU) throws {
        if cache == nil {
            let status = CVMetalTextureCacheCreate(kCFAllocatorDefault, nil, gpu.device, nil, &cache)
            guard status == kCVReturnSuccess else { throw VideoDecodeError.decoder(status) }
        }
        func texture(_ buffer: CVPixelBuffer, _ format: MTLPixelFormat, _ width: Int, _ height: Int, _ plane: Int) throws -> CVMetalTexture {
            var image: CVMetalTexture?
            let status = CVMetalTextureCacheCreateTextureFromImage(kCFAllocatorDefault, cache!, buffer, nil,
                format, width, height, plane, &image)
            guard status == kCVReturnSuccess, let image = image else { throw VideoDecodeError.decoder(status) }
            return image
        }
        let y = try texture(input, .r8Unorm, CVPixelBufferGetWidth(input), CVPixelBufferGetHeight(input), 0)
        let uv = try texture(input, .rg8Unorm, CVPixelBufferGetWidth(input)/2, CVPixelBufferGetHeight(input)/2, 1)
        let target = try texture(output, .bgra8Unorm, poolWidth, poolHeight, 0)
        guard let yt = CVMetalTextureGetTexture(y), let uvt = CVMetalTextureGetTexture(uv),
              let ot = CVMetalTextureGetTexture(target), let command = gpu.queue.makeCommandBuffer(),
              let encoder = command.makeComputeCommandEncoder() else { throw VideoDecodeError.invalidFrame }
        encoder.setComputePipelineState(gpu.pipeline)
        encoder.setTexture(yt, index: 0); encoder.setTexture(uvt, index: 1); encoder.setTexture(ot, index: 2)
        encoder.dispatchThreadgroups(MTLSize(width: (poolWidth+15)/16, height: (poolHeight+15)/16, depth: 1),
            threadsPerThreadgroup: MTLSize(width: 16, height: 16, depth: 1))
        encoder.endEncoding(); command.commit(); command.waitUntilCompleted()
        // Retain CoreVideo texture wrappers until the command has finished.
        withExtendedLifetime((y, uv, target)) {}
        guard command.status == .completed else { throw VideoDecodeError.invalidFrame }
    }
    private func software(_ input: CVPixelBuffer, into output: CVPixelBuffer) throws {
        guard CVPixelBufferLockBaseAddress(input, .readOnly) == kCVReturnSuccess else { throw VideoDecodeError.invalidFrame }
        defer { CVPixelBufferUnlockBaseAddress(input, .readOnly) }
        guard CVPixelBufferLockBaseAddress(output, []) == kCVReturnSuccess else { throw VideoDecodeError.invalidFrame }
        defer { CVPixelBufferUnlockBaseAddress(output, []) }
        guard let yp = CVPixelBufferGetBaseAddressOfPlane(input, 0)?.assumingMemoryBound(to: UInt8.self),
              let uv = CVPixelBufferGetBaseAddressOfPlane(input, 1)?.assumingMemoryBound(to: UInt8.self),
              let dst = CVPixelBufferGetBaseAddress(output)?.assumingMemoryBound(to: UInt8.self) else { throw VideoDecodeError.invalidFrame }
        let ys = CVPixelBufferGetBytesPerRowOfPlane(input, 0), us = CVPixelBufferGetBytesPerRowOfPlane(input, 1)
        let stride = CVPixelBufferGetBytesPerRow(output), ew = CVPixelBufferGetWidth(input)
        let w = poolWidth + poolWidth%2, n = w*(poolHeight+poolHeight%2)
        for y in 0..<poolHeight { for x in 0..<poolWidth {
            let i = y*w+x, ui = n+i, c = Int(yp[i/ew*ys+i%ew])-16
            let d = Int(yp[ui/ew*ys+ui%ew])-128, e = Int(uv[i/ew*us+i%ew])-128
            let offset = y*stride+x*4
            dst[offset] = UInt8(clamping: (298*c+516*d+128)>>8)
            dst[offset+1] = UInt8(clamping: (298*c-100*d-208*e+128)>>8)
            dst[offset+2] = UInt8(clamping: (298*c+409*e+128)>>8)
            dst[offset+3] = 255
        }}
    }
}
