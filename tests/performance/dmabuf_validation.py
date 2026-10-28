#!/usr/bin/env python3
"""Validate a private GBM/XRender target -> DMA-buf -> VA conversion/H264 path.

Creates no windows and reads no user drawable. This is a serialized standalone
prototype; its endpoint is encoded packet availability, not the Agent socket.
"""
import argparse
import ctypes as C
import json
import os
from pathlib import Path
import subprocess
import time

import cairo


def bind(lib, name, result, *args):
    fn = getattr(lib, name)
    fn.restype, fn.argtypes = result, args
    return fn


P, U, I, L = C.c_void_p, C.c_uint, C.c_int, C.c_ulong


class Cookie(C.Structure):
    _fields_ = [('sequence', U)]


class Color(C.Structure):
    _fields_ = [(k, C.c_ushort) for k in ('red', 'green', 'blue', 'alpha')]


class PixmapReply(C.Structure):
    _fields_ = [('response', C.c_ubyte), ('fds', C.c_ubyte), ('sequence', C.c_ushort),
                ('length', U), ('size', U), ('width', C.c_ushort), ('height', C.c_ushort),
                ('stride', C.c_ushort), ('depth', C.c_ubyte), ('bpp', C.c_ubyte),
                ('padding', C.c_ubyte * 12)]


class Canvas:
    def __init__(self, width, height, alignment=1, await_fence=False):
        self.width, self.height = width, height
        self.await_fence = await_fence
        self.x = C.CDLL('libX11.so.6')
        self.r = C.CDLL('libXrender.so.1')
        self.s = C.CDLL('libXext.so.6')
        self.g = C.CDLL('libgbm.so.1')
        self.c = C.CDLL('libxcb.so.1')
        self.d = C.CDLL('libxcb-dri3.so.0')
        self.xc = C.CDLL('libX11-xcb.so.1')
        self.libc = C.CDLL(None)
        self.display = bind(self.x, 'XOpenDisplay', P, C.c_char_p)(None)
        if not self.display:
            raise RuntimeError('a local GPU X11 display is required; only private pixmaps are accessed')
        assert bind(self.x, 'XDefaultDepth', I, P, I)(self.display, 0) == 24
        self.root = bind(self.x, 'XDefaultRootWindow', L, P)(self.display)
        self.conn = bind(self.xc, 'XGetXCBConnection', P, P)(self.display)
        self.device_fd = os.open('/dev/dri/renderD128', os.O_RDWR | os.O_CLOEXEC)
        self.device = bind(self.g, 'gbm_create_device', P, I)(self.device_fd)
        self.bo = bind(self.g, 'gbm_bo_create', P, P, U, U, U, U)(
            self.device, (width + alignment - 1) // alignment * alignment,
            height, int.from_bytes(b'XR24', 'little'), 20)
        assert self.bo, 'linear GBM target unavailable'
        self.fd = bind(self.g, 'gbm_bo_get_fd', I, P)(self.bo)
        self.stride = bind(self.g, 'gbm_bo_get_stride', U, P)(self.bo)
        self.modifier = bind(self.g, 'gbm_bo_get_modifier', C.c_uint64, P)(self.bo)
        assert self.fd >= 0 and self.stride >= width * 4 and self.modifier == 0
        assert os.fstat(self.fd).st_size >= self.stride * height
        self.target = bind(self.c, 'xcb_generate_id', U, P)(self.conn)
        # libxcb takes ownership of the submitted duplicate descriptor.
        cookie = bind(self.d, 'xcb_dri3_pixmap_from_buffer_checked', Cookie,
                      P, U, U, U, C.c_ushort, C.c_ushort, C.c_ushort,
                      C.c_ubyte, C.c_ubyte, I)(
            self.conn, self.target, self.root, self.stride * height,
            width, height, self.stride, 24, 32, os.dup(self.fd))
        error = bind(self.c, 'xcb_request_check', P, P, Cookie)(self.conn, cookie)
        if error:
            code = C.string_at(error, 2)[1]
            bind(self.libc, 'free', None, P)(error)
            raise RuntimeError(f'private DRI3 import failed: {code}')
        self.source = bind(self.x, 'XCreatePixmap', L, P, L, U, U, U)(
            self.display, self.root, width, height, 24)
        fmt = bind(self.r, 'XRenderFindStandardFormat', P, P, I)(self.display, 1)
        create = bind(self.r, 'XRenderCreatePicture', L, P, L, P, L, P)
        self.source_picture = create(self.display, self.source, fmt, 0, None)
        self.target_picture = create(self.display, self.target, fmt, 0, None)
        self.fence = bind(self.s, 'XSyncCreateFence', L, P, L, I)(self.display, self.target, 0)
        self.fence_used = False
        self._prepare_pattern()
        self.fill = bind(self.r, 'XRenderFillRectangle', None,
                         P, I, L, C.POINTER(Color), I, I, U, U)
        self.composite = bind(self.r, 'XRenderComposite', None,
                              P, I, L, L, L, I, I, I, I, I, I, U, U)
        self.reset = bind(self.s, 'XSyncResetFence', I, P, L)
        self.trigger = bind(self.s, 'XSyncTriggerFence', I, P, L)
        self.await_sync = bind(self.s, 'XSyncAwaitFence', I, P, C.POINTER(L), I)
        self.sync = bind(self.x, 'XSync', I, P, I)
        self.export = bind(self.d, 'xcb_dri3_buffer_from_pixmap', Cookie, P, U)
        self.export_reply = bind(self.d, 'xcb_dri3_buffer_from_pixmap_reply',
                                 C.POINTER(PixmapReply), P, Cookie, C.POINTER(P))
        self.export_fds = bind(self.d, 'xcb_dri3_buffer_from_pixmap_reply_fds',
                               C.POINTER(I), P, C.POINTER(PixmapReply))

    def _prepare_pattern(self):
        surface = cairo.ImageSurface(cairo.FORMAT_RGB24, self.width, self.height)
        ctx = cairo.Context(surface)
        gradient = cairo.LinearGradient(0, 0, self.width, self.height)
        gradient.add_color_stop_rgb(0, .15, .35, .8)
        gradient.add_color_stop_rgb(1, .95, .7, .3)
        ctx.set_source(gradient)
        ctx.paint()
        ctx.set_font_size(17)
        for y in range(58, self.height, 32):
            for x in range(12, self.width, 512):
                ctx.set_source_rgb(.04, .04, .04)
                ctx.move_to(x, y)
                ctx.show_text(f'GPU shared buffer   line {y // 32:04d}')
                ctx.set_source_rgb(.75, .85, .9)
                ctx.rectangle(x, y + 5, 440, 2)
                ctx.fill()
        data = bytes(surface.get_data())
        pointer = bind(self.libc, 'malloc', P, C.c_size_t)(len(data))
        assert pointer
        C.memmove(pointer, data, len(data))
        visual = bind(self.x, 'XDefaultVisual', P, P, I)(self.display, 0)
        image = bind(self.x, 'XCreateImage', P, P, P, U, I, I, P, U, U, I, I)(
            self.display, visual, 24, 2, 0, pointer, self.width, self.height, 32, self.width * 4)
        assert image
        gc = bind(self.x, 'XCreateGC', P, P, L, L, P)(self.display, self.source, 0, None)
        bind(self.x, 'XPutImage', I, P, L, P, P, I, I, I, I, U, U)(
            self.display, self.source, gc, image, 0, 0, 0, 0, self.width, self.height)
        bind(self.x, 'XDestroyImage', I, P)(image)
        bind(self.x, 'XFreeGC', I, P, P)(self.display, gc)
        bind(self.x, 'XSync', I, P, I)(self.display, 0)

    def draw(self, counter):
        for bit in range(8):
            value = 65535 if counter & (1 << bit) else 0
            color = Color(value, value, value, 65535)
            self.fill(self.display, 1, self.source_picture, C.byref(color), bit * 32, 0, 32, 32)

    def capture(self, scroll=0):
        begin = time.monotonic_ns()
        offset = scroll % (self.height - 32) if self.height > 32 else 0
        if offset:
            for sy, dy, height in ((0, 0, 32), (32 + offset, 32, self.height - 32 - offset),
                                   (32, self.height - offset, offset)):
                self.composite(self.display, 1, self.source_picture, 0, self.target_picture,
                               0, sy, 0, 0, 0, dy, self.width, height)
        else:
            self.composite(self.display, 1, self.source_picture, 0, self.target_picture,
                           0, 0, 0, 0, 0, 0, self.width, self.height)
        if self.fence_used:
            self.reset(self.display, self.fence)
        self.trigger(self.display, self.fence)
        self.fence_used = True
        if self.await_fence:
            fence = L(self.fence)
            self.await_sync(self.display, C.byref(fence), 1)
        self.sync(self.display, 0)
        error = P()
        reply = self.export_reply(self.conn, self.export(self.conn, self.target), C.byref(error))
        assert reply and not error and reply.contents.fds == 1
        exported = self.export_fds(self.conn, reply)[0]
        try:
            original, actual = os.fstat(self.fd), os.fstat(exported)
            assert (original.st_dev, original.st_ino) == (actual.st_dev, actual.st_ino)
            assert (reply.contents.width, reply.contents.height, reply.contents.stride) == (
                self.width, self.height, self.stride)
            assert (reply.contents.depth, reply.contents.bpp) == (24, 32)
            # The legacy DRI3 reply can report visible pixel bytes rather than
            # the BO's padded allocation. Validate the exported object's size.
            assert actual.st_size >= self.stride * self.height
        finally:
            os.close(exported)
            self.libc.free(reply)
        return begin

    def close(self):
        self.sync(self.display, 0)
        bind(self.s, 'XSyncDestroyFence', I, P, L)(self.display, self.fence)
        free_picture = bind(self.r, 'XRenderFreePicture', None, P, L)
        free_picture(self.display, self.target_picture)
        free_picture(self.display, self.source_picture)
        free_pixmap = bind(self.x, 'XFreePixmap', I, P, L)
        free_pixmap(self.display, self.target)
        free_pixmap(self.display, self.source)
        bind(self.x, 'XCloseDisplay', I, P)(self.display)
        os.close(self.fd)
        bind(self.g, 'gbm_bo_destroy', None, P)(self.bo)
        bind(self.g, 'gbm_device_destroy', None, P)(self.device)
        os.close(self.device_fd)


class Native:
    def __init__(self, path):
        self.lib = C.CDLL(str(path.resolve()))
        self.input_new = bind(self.lib, 'probe_input_new', P, I, U, U, U, I)
        self.input_free = bind(self.lib, 'probe_input_free', None, P)
        self.readback = bind(self.lib, 'probe_readback', I, P, P)
        self.encoder_new = bind(self.lib, 'probe_encoder_new', P, U, U, U, U, I, C.c_char_p)
        self.encoder_free = bind(self.lib, 'probe_encoder_free', None, P)
        self.error = bind(self.lib, 'probe_encoder_error', C.c_char_p, P)
        self.input_counter = bind(self.lib, 'probe_input_counter', I, P)
        self.encode = bind(self.lib, 'probe_encode', I, P, P, C.POINTER(P),
                           C.POINTER(C.c_size_t), C.POINTER(I), C.POINTER(C.c_uint64))
        self.caps = bind(self.lib, 'probe_conversion_caps', P, P)
        self.free = bind(self.lib, 'probe_free', None, P)
        self.to_rgb = bind(self.lib, 'probe_to_rgb', None, P, P, C.c_size_t)
        self.mse = bind(self.lib, 'probe_mse', C.c_double, P, P, C.c_size_t)

    def packet(self, encoder, source):
        pointer, length, key = P(), C.c_size_t(), I()
        times = (C.c_uint64 * 3)()
        status = self.encode(encoder, source, C.byref(pointer), C.byref(length), C.byref(key), times)
        if status:
            raise RuntimeError(self.error(encoder).decode() or 'native encode failed')
        try:
            packet = C.string_at(pointer, length.value)
        finally:
            self.free(pointer)
        return packet, bool(key.value), list(times)


def threads(width, height):
    if width * height < 2_000_000:
        return 1
    limit = min(4, len(os.sched_getaffinity(0)), max(1, height // 200))
    return next((n for n in range(limit, 1, -1) if ((height + n - 1) // n) % 2 == 0), 1)


def decode(data, width, height, frames):
    result = subprocess.run(['/usr/bin/python3', '-c',
                             Path(__file__).with_name('decode_quality.py').read_text(),
                             str(width), str(height), str(frames)],
                            input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True)
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--library', type=Path, required=True)
    parser.add_argument('--width', type=int, default=640)
    parser.add_argument('--height', type=int, default=480)
    parser.add_argument('--color', choices=('auto', 'bt601', 'bt709'), default='auto')
    parser.add_argument('--frames', type=int, default=3)
    parser.add_argument('--gpu-sync', choices=('implicit', 'wait'), default='wait')
    parser.add_argument('--storage-alignment', type=int, choices=(1, 16, 32, 64), default=1,
                        help='GBM allocation width alignment in pixels; visible pixmap size stays fixed')
    parser.add_argument('--artifacts', type=Path, help='optional synthetic test images only')
    parser.add_argument('--scroll', action='store_true', help='scroll the owned pattern below the counter')
    parser.add_argument('--await-fence', action='store_true', help='await XSync rendering fence before export')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if not (256 <= args.width <= 4096 and 32 <= args.height <= 4096
            and args.width * args.height <= 8_000_000
            and args.width % 2 == 0 and args.height % 2 == 0 and 3 <= args.frames <= 12
            and args.width * args.height * args.frames * 3 <= 256_000_000):
        parser.error('requires even protocol dimensions, 3..12 frames, at most 256 MB of RGB references')
    native = Native(args.library)
    canvas = Canvas(args.width, args.height, args.storage_alignment, args.await_fence)
    inputs, encoders = [], []
    try:
        for cpu in (True, False):
            source = native.input_new(canvas.fd, args.width, args.height, canvas.stride, cpu)
            assert source
            inputs.append(source)
        for mode in (0, 3 if args.gpu_sync == 'wait' else 2):
            encoder = native.encoder_new(args.width, args.height, 60,
                                         threads(args.width, args.height), mode,
                                         b'' if args.color == 'auto' else args.color.encode())
            assert encoder
            encoders.append(encoder)
            assert not native.error(encoder)
        streams = [bytearray(), bytearray()]
        originals = bytearray()
        raw = C.create_string_buffer(args.width * args.height * 4)
        rgb = C.create_string_buffer(args.width * args.height * 3)
        timings = []
        for sequence in range(args.frames):
            canvas.draw(sequence)
            begin = canvas.capture(sequence * 11 if args.scroll else 0)
            # GPU encode happens before any CPU readback/reference sync.
            packet, key, points = native.packet(encoders[1], inputs[1])
            assert key == (sequence % 60 == 0)
            streams[1].extend(packet)
            assert native.readback(inputs[0], raw) == 0
            native.to_rgb(raw, rgb, args.width * args.height)
            originals.extend(rgb.raw)
            packet, key, _ = native.packet(encoders[0], inputs[0])
            assert key == (sequence % 60 == 0)
            streams[0].extend(packet)
            timings.append(dict(sequence=sequence, render_begin_ns=begin,
                                wrap_begin_ns=points[0], wrapped_ns=points[1], encoded_ns=points[2]))
        decoded = [decode(bytes(data), args.width, args.height, args.frames) for data in streams]
        frame_bytes = args.width * args.height * 3
        if args.artifacts:
            from PIL import Image
            args.artifacts.mkdir(exist_ok=False)
            for name, data in [('reference', bytes(originals)), ('software', decoded[0]), ('gpu', decoded[1])]:
                Image.frombytes('RGB', (args.width, args.height), data[frame_bytes:frame_bytes*2]).save(args.artifacts/(name+'.png'))
        errors = []
        for sequence in range(args.frames):
            start, end = sequence * frame_bytes, (sequence + 1) * frame_bytes
            reference = bytes(originals[start:end])
            mse = [native.mse(C.c_char_p(reference), C.c_char_p(data[start:end]), frame_bytes) for data in decoded]
            markers = [sum((data[start + (16 * args.width + bit * 32 + 16) * 3] > 128) << bit
                           for bit in range(8)) for data in decoded]
            errors.append(dict(sequence=sequence, software_mse=mse[0], dmabuf_mse=mse[1], markers=markers))
        caps = []
        for encoder in encoders:
            pointer = native.caps(encoder)
            caps.append(C.string_at(pointer).decode())
            native.free(pointer)
        result = dict(schema=1, dimensions=[args.width, args.height], frames=args.frames,
                      target_stride=canvas.stride, color=args.color, gpu_sync=args.gpu_sync, quality=errors,
                      storage_alignment=args.storage_alignment,
                      scroll=args.scroll,
                      await_fence=args.await_fence,
                      quality_pass=all(row['markers'] == [row['sequence'] % 256] * 2
                                       and row['dmabuf_mse'] <= row['software_mse'] * 1.02 + .01 for row in errors),
                      conversion_caps=caps, gpu_timings=timings,
                      limitation='Standalone serialized private pixmap prototype, not Agent publication latency')
        args.output.write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps({k:v for k,v in result.items() if k != 'gpu_timings'}), flush=True)
        if not result['quality_pass']:
            raise RuntimeError('decoded quality or visual sequence check failed; see saved result')
    finally:
        for encoder in encoders:
            native.encoder_free(encoder)
        for source in inputs:
            native.input_free(source)
        canvas.close()


if __name__ == '__main__':
    main()
