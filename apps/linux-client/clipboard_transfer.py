"""Legacy Linux binary clipboard transport on an independent paired QUIC stream."""
import base64
import json
import os
import select
import subprocess
import threading
import time
from wire import STATE,route
from runtime import helper_binary
import clipboard


class Transfer:
    def __init__(self,peer,operation,transfer=None):
        route(peer)  # Verify this is a currently paired device before spawning.
        args=[str(helper_binary('paneferry-network')),operation,'--state',str(STATE/'network'),'--peer',peer[5:]]
        if transfer is not None:
            if not isinstance(transfer,str) or not 0<len(transfer)<=100 or not all(c.isascii() and (c.isalnum() or c in '_-') for c in transfer):
                raise ValueError('clipboard_invalid')
            args+=['--transfer',transfer]
        self.process=subprocess.Popen(args,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,bufsize=0)
        self.buffer=bytearray();self.deadline=time.monotonic()+30

    def fill(self):
        wait=self.deadline-time.monotonic()
        if wait<=0 or not select.select([self.process.stdout],[],[],wait)[0]: raise ValueError('clipboard_transfer_timeout')
        data=os.read(self.process.stdout.fileno(),65536)
        if not data:raise ValueError('clipboard_invalid')
        self.buffer.extend(data)

    def line(self,limit):
        while True:
            index=self.buffer.find(b'\n')
            if index>=0:
                if index>limit:raise ValueError('clipboard_invalid')
                data=bytes(self.buffer[:index]);del self.buffer[:index+1];return data
            if len(self.buffer)>limit:raise ValueError('clipboard_invalid')
            self.fill()

    def read(self,count):
        result=[]
        while count:
            if not self.buffer:self.fill()
            take=min(count,len(self.buffer));result.append(bytes(self.buffer[:take]));del self.buffer[:take];count-=take
        return b''.join(result)

    def finish(self):
        if self.buffer:raise ValueError('clipboard_invalid')
        if self.process.wait(timeout=max(.1,self.deadline-time.monotonic()))!=0:raise ValueError('clipboard_invalid')
        if self.process.stdout.read(1):raise ValueError('clipboard_invalid')

    def close(self):
        if self.process.poll() is None:self.process.kill()
        self.process.wait(timeout=2)
        self.process.stdout.close()
        if not self.process.stdin.closed:self.process.stdin.close()


def upload(peer,bundle):
    values=clipboard.validate(bundle)
    if bundle['kind'] not in ('image','files'):raise ValueError('clipboard_invalid')
    manifest=dict(kind=bundle['kind'],entries=[dict(path=name,directory=folder,size=len(data)) for name,folder,data in values])
    data=json.dumps(manifest,separators=(',',':')).encode()+b'\n'+b''.join(data for _,folder,data in values if not folder)
    transfer=Transfer(peer,'bulk-upload')
    failures=[]
    def write():
        try:
            remaining=memoryview(data)
            while remaining:
                count=transfer.process.stdin.write(remaining)
                if not count:raise ValueError('clipboard_invalid')
                remaining=remaining[count:]
        except (OSError,ValueError) as error:failures.append(error)
        finally:transfer.process.stdin.close()
    sender=threading.Thread(target=write,daemon=True);sender.start()
    try:
        identity=transfer.line(100).decode('ascii')
        if not identity or not all(c.isalnum() or c in '_-' for c in identity):raise ValueError('clipboard_invalid')
        transfer.finish();sender.join(timeout=1)
        if failures or sender.is_alive():raise ValueError('clipboard_invalid')
        return identity
    finally:transfer.close();sender.join(timeout=1)


def download(peer,identity):
    transfer=Transfer(peer,'bulk-download',identity)
    transfer.process.stdin.close()
    try:
        bundle=json.loads(transfer.line(512*1024))
        if not isinstance(bundle,dict):raise ValueError('clipboard_invalid')
        entries=bundle.get('entries')
        if bundle.get('kind') not in ('image','files') or not isinstance(entries,list) or not 0<len(entries)<=256:raise ValueError('clipboard_invalid')
        total=0
        for entry in entries:
            if not isinstance(entry,dict):raise ValueError('clipboard_invalid')
            size=entry.get('size'); path=entry.get('path'); folder=entry.get('directory',False)
            if type(size) is not int or not 0<=size<=clipboard.MAX-total or not isinstance(path,str) or len(path.encode())>1024 or type(folder) is not bool:
                raise ValueError('clipboard_invalid')
            if folder and size:raise ValueError('clipboard_invalid')
            total+=size
        for entry in entries:
            entry.setdefault('directory',False)
            if not entry['directory']:entry['data']=base64.b64encode(transfer.read(entry['size'])).decode()
        transfer.finish();clipboard.validate(bundle)
        return bundle
    finally:transfer.close()
