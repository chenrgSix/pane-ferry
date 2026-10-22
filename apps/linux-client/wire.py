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
    'network_operation_failed': '配对操作未完成，请检查设备地址、邀请有效期和设备身份。',
    'network_unavailable': '直接连接组件未就绪，请检查 UDP 42424 端口。',
    'sharing_disabled': '对方已停止共享，应用仍在原设备运行。',
    'screen_recording_required': '请在 Mac 的 LAN RemoteApp 中允许录屏。',
    'accessibility_required': '请在 Mac 的 LAN RemoteApp 中允许辅助功能。',
    'session_exited': '原应用窗口已关闭。', 'session_expired': '原会话已结束，请刷新窗口列表。',
    'session_controlled': '此窗口已有控制端，请先断开原连接。',
    'desktop_secrets_unavailable': '应用需要 Ubuntu 桌面密钥服务，请确认桌面已登录且已安装 xdg-dbus-proxy。',
    'desktop_locked': 'Mac 桌面已锁定，解锁后重连。',
    'clipboard_invalid': '剪贴板格式不可用或超过 8 MiB / 256 项限制。',
    'clipboard_copy_failed': '应用没有提供选中的内容。',
    'capture_failed': 'Mac 窗口采集失败，请检查录屏权限。',
    'window_unavailable': '原窗口暂时不可用，请刷新后重试。',
}


def message(error):
    # Never show arbitrary remote strings, paths, stderr or private content.
    return MESSAGES.get(str(error), '连接暂时不可用，正在等待设备恢复。')
