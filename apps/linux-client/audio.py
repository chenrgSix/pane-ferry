"""One independent system-audio subscription/player per paired device."""
import json
import select
import subprocess
import threading
import time
from wire import Wire
from runtime import helper_binary

CAPABILITY='audio.system.opus.v1'


class Subscription:
    def __init__(self,peer,changed):
        self.peer=peer; self.changed=changed; self.stop=threading.Event(); self.wire=None
        self.thread=threading.Thread(target=self.run,daemon=True); self.thread.start()

    def close(self):
        self.stop.set()
        if self.wire: self.wire.close()

    def player_read(self,player):
        # Pipes are unbuffered, so select observes every pending JSON reply.
        if not select.select([player.stdout],[],[],2)[0]: raise ValueError('声音播放器没有响应')
        line=player.stdout.readline(8193)
        if not line.endswith(b'\n') or len(line)>8192: raise ValueError('声音播放器已退出')
        return json.loads(line)

    def run(self):
        while not self.stop.is_set():
            player=None; wire=None
            try:
                self.changed(self.peer,'connecting','正在连接系统声音')
                wire=Wire(self.peer); self.wire=wire
                if self.stop.is_set(): break
                if CAPABILITY not in wire.capabilities: raise ValueError('发布端尚不支持系统声音，请更新发布端')
                player=subprocess.Popen([str(helper_binary('paneferry-agent')),'audio-play'],
                    stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,bufsize=0)
                if self.player_read(player).get('type')!='audio.ready': raise ValueError('声音播放器不可用')
                wire.send(type='audio.subscribe'); format=wire.until('audio.format')
                if any(format.get(k)!=v for k,v in dict(codec='opus',sample_rate=48000,channels=2,frame_samples=480).items()):
                    raise ValueError('声音格式不受支持')
                self.changed(self.peer,'playing','正在接收系统声音')
                started=time.monotonic(); minimum=None; adjusted_at=0; last=0
                while not self.stop.is_set():
                    event=wire.read(timeout=5); kind=event.get('type')
                    if kind=='audio.heartbeat': continue
                    if kind!='audio.frame': raise ValueError('远端声音已停止，将自动重试')
                    sequence,stamp=event.get('sequence'),event.get('timestamp_ms')
                    if type(sequence) is not int or not last<sequence<2**64 or type(stamp) is not int or not 0<=stamp<2**63:
                        raise ValueError('声音序列无效')
                    last=sequence
                    arrival=(time.monotonic()-started)*1000
                    offset=arrival-stamp
                    if minimum is None: minimum=offset
                    drift=int((arrival-adjusted_at)//1000)
                    if drift>0: minimum+=drift; adjusted_at=arrival
                    minimum=min(minimum,offset)
                    if offset-minimum>80:
                        wire.send(type='audio.ack',sequence=sequence); continue
                    data=event.get('data')
                    if not isinstance(data,str) or not 0<len(data)<=1700: raise ValueError('声音数据无效')
                    packet=(json.dumps(dict(type='audio.frame',sequence=sequence,data=data))+'\n').encode()
                    if player.stdin.write(packet)!=len(packet): raise ValueError('声音播放器已退出')
                    reply=self.player_read(player)
                    if reply.get('sequence')!=sequence or reply.get('type')!='audio.ack': raise ValueError('声音播放失败')
                    reply.pop('version',None); wire.send(**reply)
            except (OSError,ValueError,EOFError,KeyError,subprocess.SubprocessError) as error:
                if not self.stop.is_set(): self.changed(self.peer,'unavailable',str(error))
            finally:
                self.wire=None
                if wire: wire.close()
                if player:
                    player.stdin.close()
                    try: player.wait(timeout=2)
                    except subprocess.TimeoutExpired: player.kill(); player.wait(timeout=2)
                    player.stdout.close()
            self.stop.wait(3)


class Service:
    def __init__(self,changed):
        self.changed=changed; self.subscriptions={}; self.generations={}

    def set(self,peer,enabled):
        if enabled and peer not in self.subscriptions:
            generation=self.generations.get(peer,0)+1; self.generations[peer]=generation
            def changed(peer,state,text):
                if self.generations.get(peer)==generation: self.changed(peer,state,text)
            self.subscriptions[peer]=Subscription(peer,changed)
        elif not enabled and peer in self.subscriptions:
            self.generations[peer]=self.generations.get(peer,0)+1
            self.subscriptions.pop(peer).close(); self.changed(peer,'off','声音已关闭')

    def stop(self):
        for peer in list(self.subscriptions): self.set(peer,False)
