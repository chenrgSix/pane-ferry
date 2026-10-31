# Windows whole-desktop sessions

Windows now implements the existing `desktop.list` / `desktop.start` protocol.
The launcher’s “整桌面 → 连接桌面” selects one currently connected monitor.
This is a monitor capture, including the taskbar, desktop and ordinary system
dialogs, rather than an enumeration/composition of application windows.

Monitor targets have an explicit identity separate from application HWNDs.
The publisher resolves IDs locally, checks the interactive desktop and current
monitor identity, and reuses sessions on duplicate requests. A monitor is never
offered through automatic application-window takeover. Unplugged targets retire
their session; a new display selection resolves a fresh target.

WGC `CreateForMonitor` supplies the shared D3D11 compositor and the existing
H.264/4:4:4 encoding paths, including CPU conversion and software encoding.
The monitor cursor is captured. Encoded canvases remain bounded to 4096 per axis
and 8 million pixels; larger monitors are scaled proportionally with even edges.
Raw monitor capture is limited to 8192 per axis and 32 million pixels. Selecting
a display does not change its resolution. Input uses the displayed canvas to
map back to physical monitor coordinates, including negative desktop origins.

Mac treats the existing `surface_type: desktop` metadata as a fixed canvas.
The Windows receiver also disables source resizing in desktop mode and accepts
portrait screens within the same pixel budget. Closing the receiver detaches;
`window.close.request` and `window.configure` cannot close or resize the desktop.

The publisher stays at its existing privilege level. Windows secure desktop,
lock screen and elevated-process input restrictions still apply. Secure desktop
stops capture/control with `desktop_locked`; the receiver tells the user to
handle the prompt locally and reconnect. Organization-managed firewall buttons
remain subject to Windows policy even when the ordinary dialog is visible.

Validation:
- Real paired QUIC protocol test: monitor enumeration/start/resume, request
  idempotency and conflicts, desktop metadata, session listing, detach/reuse,
  unknown monitor rejection, and close/resize rejection.
- Actual 2560×1440 monitor: eight full-chroma frames with GPU texture encoding;
  repeated with GPU composition disabled and software encoding. No desktop
  pixels saved. Coordinate routing and invalid monitor identity checks passed.
- Large landscape/portrait and negative-origin canvas bounds tested.
- Existing Windows build/backend/network gates and launcher tests passed.
- Mac smoke coverage added for desktop metadata, local-only resize and
  detach-only close; cross-machine interaction still needs user validation.
