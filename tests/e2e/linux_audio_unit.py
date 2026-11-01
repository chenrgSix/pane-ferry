#!/usr/bin/env python3
"""Audio lifetime races without starting a capture, player or network connection."""
from pathlib import Path
import sys
import threading
from types import SimpleNamespace
sys.path.insert(0,str(Path(__file__).resolve().parents[2]/'apps/linux-client'))
import audio

closed=[]
class Finishing(audio.Subscription):
    def __init__(self):self.stop=threading.Event();self._wire=SimpleNamespace(close=lambda:closed.append(True))
    @property
    def wire(self):
        value=self._wire;self._wire=None;return value
subscription=Finishing();subscription.close()
assert subscription.stop.is_set() and closed==[True]
created=[];changes=[]
class Fake:
    def __init__(self,peer,changed):self.changed=changed;self.closed=False;created.append(self)
    def close(self):self.closed=True
audio.Subscription=Fake
service=audio.Service(lambda *value:changes.append(value))
assert not service.subscriptions
service.set('one',True);service.set('one',True);service.set('two',True)
assert len(created)==2
first=created[0];service.set('one',False)
assert first.closed and not created[1].closed
before=len(changes);first.changed('one','playing','late status')
assert len(changes)==before,'an old generation must not overwrite the disabled/new player'
service.set('one',True);service.stop()
assert not service.subscriptions and all(value.closed for value in created)
print('PASS audio shutdown race, per-device deduplication, stale-generation rejection and complete stop')
