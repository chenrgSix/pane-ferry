"""Private per-application lifecycle IPC shared with the Tauri launcher.

Only attach/list/detach metadata uses this socket. Video, input and clipboard
continue to use the existing authenticated QUIC receiver path.
"""
import json
import os
from pathlib import Path
import re
import socket
import stat
import struct
import threading


def valid_id(value):
    return isinstance(value, str) and not value.startswith('-') and re.fullmatch(r'[A-Za-z0-9_.-]{1,128}', value) is not None


def validate(request, group):
    if not isinstance(request, dict) or type(request.get('version')) is not int or request.get('version') != 1 or request.get('group') != group:
        raise ValueError('invalid_broker_request')
    op = request.get('op')
    if op not in ('attach', 'detach', 'list'):
        raise ValueError('invalid_broker_operation')
    if op != 'list' and not valid_id(request.get('session')):
        raise ValueError('invalid_session')
    if op == 'attach':
        if request.get('target') is not None and not valid_id(request['target']):
            raise ValueError('invalid_target')
        if type(request.get('fps')) is not int or request['fps'] not in (30, 60):
            raise ValueError('invalid_fps')
        if request.get('picture_mode') not in ('adapt', 'source') or request.get('video_color') not in ('420', '444-atlas-v1'):
            raise ValueError('invalid_presentation')
        if type(request.get('background')) is not bool:
            raise ValueError('invalid_background')
    return request


class Broker:
    def __init__(self, group, handle):
        if not isinstance(group, str) or re.fullmatch('[a-f0-9]{64}', group) is None:
            raise ValueError('invalid_group')
        self.group, self.handle = group, handle
        self.directory = Path('/tmp') / f'pf-app-{os.getuid()}'
        self.directory.mkdir(mode=0o700, exist_ok=True)
        metadata = self.directory.lstat()
        if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
            raise ValueError('invalid_broker_directory')
        self.path = self.directory / (group + '.sock')
        self.listener = socket.socket(socket.AF_UNIX)
        self.listener.bind(str(self.path))  # A live owner's socket is never removed.
        self.path.chmod(0o600)
        self.inode = self.path.stat().st_ino
        self.listener.listen(16)
        self.stopped = threading.Event()
        self.connections = set(); self.lock = threading.Lock()
        self.slots = threading.BoundedSemaphore(16)
        threading.Thread(target=self.serve, daemon=True).start()

    @staticmethod
    def send(client, value):
        data = json.dumps(dict(version=1, **value), separators=(',', ':')).encode() + b'\n'
        if len(data) > 16 * 1024: raise ValueError('broker_response_too_large')
        client.sendall(data)

    def serve(self):
        while not self.stopped.is_set():
            try: client, _ = self.listener.accept()
            except OSError: return
            if not self.slots.acquire(blocking=False):
                client.close(); continue
            with self.lock: self.connections.add(client)
            threading.Thread(target=self.client, args=(client,), daemon=True).start()

    def client(self, client):
        keep = False
        try:
            client.settimeout(5)
            credentials = client.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize('3i'))
            if struct.unpack('3i', credentials)[1] != os.getuid(): raise ValueError('broker_peer_mismatch')
            # Read exactly one bounded request; never consume a watcher's data.
            data = bytearray()
            while len(data) < 16 * 1024:
                byte = client.recv(1)
                if not byte: raise EOFError('broker_closed')
                if byte == b'\n': break
                data.extend(byte)
            else: raise ValueError('broker_request_too_large')
            request = validate(json.loads(data), self.group)
            keep = self.handle(request, client)
        except (OSError, ValueError, EOFError, TimeoutError):
            pass
        finally:
            if not keep: self.drop(client)
            self.slots.release()

    def drop(self, client):
        with self.lock: self.connections.discard(client)
        try: client.shutdown(socket.SHUT_RDWR)
        except OSError: pass
        client.close()

    def close(self):
        self.stopped.set()
        try: self.listener.shutdown(socket.SHUT_RDWR)
        except OSError: pass
        self.listener.close()
        with self.lock: clients = list(self.connections)
        for client in clients: self.drop(client)
        try:
            if self.path.lstat().st_ino == self.inode: self.path.unlink()
        except FileNotFoundError: pass
