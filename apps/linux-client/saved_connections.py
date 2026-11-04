"""Local device bookmarks; pairing trust remains in the network service."""
import ipaddress
import json
import re
import uuid


def normalize(value):
    identity = value['peerId']
    if not isinstance(identity, str) or str(uuid.UUID(identity)) != identity:
        raise ValueError('请先配对设备，再保存连接。')
    name = value['name'].strip()
    if not name or len(name) > 80:
        raise ValueError('请填写 1 至 80 个字符的连接名称。')
    match = re.fullmatch(r'(\[[0-9a-fA-F:.]+\]|[0-9.]+):([0-9]{1,5})', value['address'].strip())
    if not match or not 1 <= int(match[2]) <= 65535:
        raise ValueError('请填写有效的 IP:端口；IPv6 使用 [地址]:端口。')
    host = ipaddress.ip_address(match[1].strip('[]'))
    if match[1].startswith('[') != (host.version == 6):
        raise ValueError('IPv6 使用 [地址]:端口，IPv4 使用 IP:端口。')
    host = f'[{host}]' if host.version == 6 else str(host)
    return dict(peerId=identity, name=name, address=f'{host}:{int(match[2])}')


class SavedConnections:
    def __init__(self, path, writer):
        self.path, self.writer, self.entries = path, writer, []
        try:
            if path.is_symlink() or path.stat().st_size > 128 * 1024:
                return
            values = json.loads(path.read_text(encoding='utf-8'))
            if not isinstance(values, list):
                return
            for value in values[:100]:
                try:
                    item = normalize(value)
                    if not any(row['peerId'] == item['peerId'] for row in self.entries):
                        self.entries.append(item)
                except (ValueError, TypeError, KeyError, AttributeError):
                    pass
        except (OSError, ValueError):
            pass

    def save(self, value):
        item = normalize(value)
        exists = any(row['peerId'] == item['peerId'] for row in self.entries)
        if not exists and len(self.entries) >= 100:
            raise ValueError('最多保存 100 台设备，请先移除不再使用的连接。')
        entries = [item if row['peerId'] == item['peerId'] else row for row in self.entries] if exists else [*self.entries, item]
        self._commit(entries)

    def remove(self, identity):
        self._commit([row for row in self.entries if row['peerId'] != identity])

    def _commit(self, entries):
        try:
            self.writer(self.path, entries)
        except OSError as error:
            raise ValueError('连接保存失败，原记录已保留。请检查本机存储后重试。') from error
        self.entries = entries
