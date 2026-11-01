"""Negotiated atlas layouts shared with the Windows and Mac wire format."""
MODES = ('420', '444-atlas-v1', '444-atlas-v2')


def carrier_size(width, height, color='420'):
    if (color not in MODES or type(width) is not int or type(height) is not int
            or not 0 < width <= 4096 or not 0 < height <= 4096 or width * height > 8_000_000):
        raise ValueError('invalid_video')
    w, h = width + width % 2, height + height % 2
    if color == '420': return w, h
    if color == '444-atlas-v2' and h <= 2048:
        ew, eh = w, h * 2
    else:
        ew = min(w * 2, 4096)
        rows = (2 * w * h + ew - 1) // ew
        eh = rows + rows % 2
    if ew > 4096 or eh > 4096 or ew * eh > 16_000_000:
        raise ValueError('invalid_video')
    return ew, eh


def negotiate(requested, capabilities):
    if requested == '420': return '420'
    if requested not in MODES: raise ValueError('invalid_video')
    for color in ('444-atlas-v2', '444-atlas-v1'):
        if 'video.color.' + color in capabilities: return color
    return '420'


def unpack_nv12(raw, stride, width, height, color):
    """Return aligned Y444 input for GStreamer; no RGB matrix in Python loops."""
    ew, eh = carrier_size(width, height, color)
    if color == '420' or not ew <= stride <= ew + 256 or len(raw) != stride * eh * 3 // 2:
        raise ValueError('invalid_video')
    if stride != ew:
        raw = b''.join(raw[row * stride:row * stride + ew] for row in range(eh * 3 // 2))
    w, h = width + width % 2, height + height % 2
    n, area = w * h, ew * eh
    planes = (raw[:n], raw[n:2*n], raw[area:area+n])
    if w % 4 == 0: return b''.join(planes)
    row_size = (w + 3) & ~3
    output = bytearray(row_size * h * 3)
    for channel, plane in enumerate(planes):
        for row in range(h):
            start = (channel * h + row) * row_size
            output[start:start+w] = plane[row*w:(row+1)*w]
    return bytes(output)
