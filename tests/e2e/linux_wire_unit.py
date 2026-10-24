#!/usr/bin/env python3
"""Actual sockets: idle desktop survives; requests and partial bodies expire."""
import sys, socket, threading, time
from pathlib import Path
from unittest.mock import patch
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'apps/linux-client'))
from wire import Wire

def pair():
    left, right = socket.socketpair()
    wire = Wire.__new__(Wire); wire.socket = left; left.setblocking(False)
    wire.buffer = bytearray(); wire.lock = threading.Lock()
    return wire, right

wire, other = pair()
received = []
thread = threading.Thread(target=lambda: received.append(wire.read())); thread.start()
time.sleep(16)
assert thread.is_alive(), 'idle stream must outlive the old 15 second timeout'
other.sendall(b'{"version":1,"type":"frame"}\n')
thread.join(2); assert received[0]['type'] == 'frame'
wire.close(); other.close()
wire, other = pair()
try:
    wire.read(timeout=.03)
    raise AssertionError('request timeout missing')
except TimeoutError: pass
# An already partial body must have an absolute deadline even with no request deadline.
wire.buffer.extend(b'{')
real_select = __import__('select').select
with patch('wire.select.select', return_value=([], [], [])) as wait:
    try:
        wire.read(); raise AssertionError('partial body timeout missing')
    except TimeoutError: pass
    assert 0 < wait.call_args.args[3] <= 15
wire.close(); other.close()
print('PASS idle stream, request deadline, partial packet deadline')
