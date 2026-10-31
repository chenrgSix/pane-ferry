# Row-aligned full-chroma transport

## Change and compatibility

The `444-atlas-v1` carrier folds source rows at `min(2*w, 4096)` bytes.
For wide sources, adjacent carrier rows can contain unrelated source regions,
making H.264 spatial and motion prediction less effective. Motion can then
look noisy while the same paused content becomes clear.

Windows now advertises `video.color.444-atlas-v2`. Updated Mac receivers prefer
it when the existing full-color preference is selected; otherwise they retain
v1 or compatible 4:2:0 negotiation. Older receivers still request v1. Updating
only Windows does not activate the new layout on an old Mac receiver.

For even-padded source dimensions `w,h`, v2 uses a `w × 2*h` carrier when
`h <= 2048`. Taller sources use the v1 dimensions. Both sides enforce the
4096-per-axis and 16,000,000-carrier-pixel limits, including padding. The byte
layout is unchanged: full Y followed by full U in carrier Y; full V in carrier
UV. Thus source-row boundaries remain aligned for the common landscape case.
Source dimensions, frame rate, bitrate policy, input coordinates and window
composition are unchanged. This remains lossy H.264 4:2:0 carrying full-resolution
chroma planes, not native H.264 4:4:4 or lossless RGB.

## Local validation

- Rust publisher/backend tests and the Windows build gate passed.
- GPU conversion matches the CPU reference byte for byte for 4:2:0, v1 and v2,
  including odd edges and tall-source fallback.
- Software encoding covers 64×48 through 4096×1900 sources. Hardware texture
  encoding covers 320×180, 1602×932 and 2560×1440 in all three modes. All 27
  exported test streams decode independently with FFmpeg, including recovery
  suffixes; none contains B frames.
- Injected hardware failure switches v2 to software with a new keyframe and
  preserves source dimensions. No particular GPU vendor is required.
- Real WGC capture of the synthetic 2562×1432 animated window produced 600
  frames in 10.504 seconds, including startup, with one startup keyframe.
  Frame-gap median/p95/p99: 16.597/22.446/27.415 ms. The 2562×2864 carrier
  decoded all 600 frames independently. This is local IPC, not Mac end-to-end.
- Synthetic Windows software and hardware v2 keyframes are checked into the
  Mac decode smoke test, alongside v1 fixtures. The hardware fixture exercises
  a 2560×2880 carrier. Negotiation tests cover old and updated publishers.

## Controlled compression comparison and limits

The same 2560×1440, 60 fps synthetic animation has a static grid and changing
32×32 tiles in its bottom 1/16 band. There are 180 frames at a 73.728 Mbps
target followed by 180 at 18.432 Mbps; each measurement excludes 60 settling
frames. Hardware v1 emitted 49.09/18.41 Mbps versus v2 8.207/4.199 Mbps.
Sampled Y/U/V PSNR in the low-rate stage improved from 37.74/37.51/36.29 dB
to 63.88/64.45/59.91 dB. These are measurements of this repeating synthetic
pattern, not a percentage improvement claim for arbitrary browser video.

For the same small-band animation, software v2 already emits about 12 Mbps
at both targets, below the reduced budget. The probe therefore requires rate
reduction only when the initial actual output exceeds that budget; it still
checks the reduced output ceiling and absence of recurring keyframes.

A separate exploratory stress run changing the bottom quarter instead showed
software output around 54.94/37.57 Mbps, exceeding the reduced target. Software
CBR is not a hard byte limit for every workload; this layout change does not
solve that broader encoder limitation. Hardware on that pattern emitted
19.35/10.16 Mbps. Real video quality and the cross-machine path still require
user validation with both updated endpoints.
