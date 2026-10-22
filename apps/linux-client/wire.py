"""Device-scoped RWP connections and private local state for the unified app."""
import json
import os
from pathlib import Path
import socket
import stat
import threading
import uuid

STATE = Path.home() / '.local/state/lan-remoteapp'
LIMIT = 16 * 1024 * 1024


def private(path):
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    if path.is_symlink() or not path.is_dir():
        raise ValueError('invalid_private_directory')
    path.chmod(0o700)


def save(path, value):
    private(path.parent)
    temporary = path.with_name('.save-' + uuid.uuid4().hex)
    try:
        with open(temporary, 'x', opener=lambda p, f: os.open(p, f, 0o600)) as out:
            json.dump(value, out, ensure_ascii=False)
            out.flush(); os.fsync(out.fileno())
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def peers():
    result = []
    path = STATE / 'network/peers.json'
    try:
        if not path.is_symlink() and path.stat().st_size <= 1024 * 1024:
            values = json.loads(path.read_text())
            for identity, peer in list(values.items())[:32]:
                if str(uuid.UUID(identity)) == identity and peer['device_id'] == identity:
                    result.append(dict(device_id=identity, connection_id='quic:'+identity,
                        name=peer['name'], address=peer['address'], transport='quic', generation=identity))
    except (OSError, ValueError, KeyError, TypeError, AttributeError):
        pass
    return result


def route(peer_id):
    # Every receiver is bound to one paired UUID. Legacy return routes are ignored.
    if not isinstance(peer_id, str) or not peer_id.startswith('quic:'):
        raise ValueError('invalid_device')
    identity = peer_id[5:]
    if str(uuid.UUID(identity)) != identity or not any(p.get('connection_id') == peer_id for p in peers()):
        raise ValueError('invalid_device')
    expected = STATE / 'network' / ('r-' + identity + '.sock')
    metadata = expected.lstat()
    if not stat.S_ISSOCK(metadata.st_mode) or metadata.st_uid != os.getuid():
        raise ValueError('invalid_route')
    return expected


class Wire:
    def __init__(self, peer_id=None, local=False):
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(15)
        self.socket.connect(str(STATE / 'agent.sock' if local else route(peer_id)))
        self.reader = self.socket.makefile('rb', buffering=65536)
        self.lock = threading.Lock()
        self.send(type='hello')
        hello = self.read()
        if hello.get('type') != 'hello':
            raise ValueError('invalid_hello')
        self.capabilities = hello.get('capabilities', [])

    def send(self, **value):
        data = json.dumps(dict(value, version=1), ensure_ascii=False, separators=(',', ':')).encode() + b'\n'
        if len(data) > LIMIT:
            raise ValueError('message_too_large')
        with self.lock:
            self.socket.sendall(data)

    def read(self):
        line = self.reader.readline(LIMIT + 1)
        if not line:
            raise EOFError('connection_lost')
        if len(line) > LIMIT or not line.endswith(b'\n'):
            raise ValueError('invalid_message')
        value = json.loads(line)
        if value.get('version') != 1:
            raise ValueError('protocol_version')
        return value

    def until(self, kind):
        for _ in range(64):
            value = self.read()
            if value.get('type') == 'error':
                raise ValueError(value.get('code', 'remote_error'))
            if value.get('type') == kind:
                return value
        raise ValueError('protocol_error')

    def close(self):
        try:
            self.socket.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        self.reader.close(); self.socket.close()


def request(peer_id, kind, expected=None, local=False, **fields):
    wire = Wire(peer_id, local=local)
    try:
        wire.send(type=kind, **fields)
        return wire.until(expected or kind)
    finally:
        wire.close()


MESSAGES = {
    'wayland_bridge_unavailable': '请在 Ubuntu 启用窗渡 GNOME 扩展，再刷新运行窗口；不会另建浏览器配置。',
    'wayland_bridge_failed': 'Wayland 窗口接口暂不可用，请检查窗渡 GNOME 扩展。',
    'wayland_bridge_outdated': 'Ubuntu 仍在运行旧版窗口扩展。保存远端工作后，注销并重新登录以启用新版；当前应用不会自动关闭。',
    'wayland_capture_unavailable': 'Wayland 窗口采集未就绪，请检查 PipeWire 与 GStreamer。',
    'resolution_limited': '请求尺寸超出 Wayland 采集范围，请缩小窗口或选择较低分辨率。',
    'display_scale_requires_restart': '原窗口保留；高清适配需保存工作后重开此应用，尚未执行。',
    'display_settings_unavailable': '会话显示密度设置不可用；原桌面设置未修改。',
    'wayland_binary_clipboard_unavailable': '当前 Wayland 窗口仅支持文字剪贴板。',
    'wayland_cut_unavailable': '当前 Wayland 窗口暂不支持远程剪切，请使用复制；远端内容未删除。',
    'wayland_text_unavailable': '当前输入框未确认输入状态，请重新聚焦或使用粘贴；输入未自动重试。',
    'wayland_text_outcome_unknown': '文本可能已输入，请先核对内容，勿重复提交；输入未自动重试。',
    'network_operation_failed': '配对操作未完成，请检查设备地址、邀请有效期和设备身份。',
    'network_unavailable': '直接连接组件未就绪，请检查 UDP 42424 端口。',
    'sharing_disabled': '对方已停止共享，应用仍在原设备运行。',
    'screen_recording_required': '请在 Mac 的 窗渡 PaneFerry 中允许录屏。',
    'accessibility_required': '请在 Mac 的 窗渡 PaneFerry 中允许辅助功能。',
    'application_multiple_windows': '该应用已有多个窗口，请从运行窗口列表选择。',
    'application_start_failed': '应用在产生窗口前已退出，请检查已有实例或启动兼容性。',
    'application_window_pending': '应用尚未产生窗口，会话已保留，请稍后重试。',
    'session_exited': '原应用窗口已关闭。', 'session_expired': '原会话已结束，请刷新窗口列表。',
    'session_controlled': '此窗口已有控制端，请先断开原连接。',
    'desktop_secrets_unavailable': '应用需要 Ubuntu 桌面密钥服务，请确认桌面已登录且已安装 xdg-dbus-proxy。',
    'desktop_locked': '远端桌面已锁定，解锁后重连。',
    'clipboard_invalid': '剪贴板格式不可用或超过 8 MiB / 256 项限制。',
    'clipboard_copy_failed': '应用没有提供选中的内容。',
    'capture_failed': 'Mac 窗口采集失败，请检查录屏权限。',
    'window_unavailable': '原窗口暂时不可用，请刷新后重试。',
}


def message(error):
    # Never show arbitrary remote strings, paths, stderr or private content.
    return MESSAGES.get(str(error), '连接暂时不可用，正在等待设备恢复。')
