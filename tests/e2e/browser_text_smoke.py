#!/usr/bin/env python3
"""Opt-in Linux Firefox regression, using an owned Xvfb and disposable profile only."""
import argparse
import base64
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import queue
import secrets
import signal
import socket
import subprocess
import tempfile
import threading
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--agent', required=True, type=Path)
parser.add_argument('--artifacts', type=Path, help='optional diagnostic directory; uses PNG instead of H.264')
args = parser.parse_args()
children = []
peers = []
state = {}
request_paths = []
scope = 'remoteapp-browser-regression-' + secrets.token_hex(5) + '.scope'

PAGE = b'''<!doctype html><meta charset="utf-8"><title>RemoteApp browser regression</title>
<style>body{margin:30px}textarea{width:650px;height:180px;font:22px sans-serif}</style>
<textarea id="entry" autofocus></textarea><button id="button">Click</button>
<script>
const entry=document.querySelector('textarea'),documentId=Math.random().toString(16); let clicks=0,lastKey='',pasted=[];
document.addEventListener('paste',e=>{
 const files=Array.from(e.clipboardData.files); if(!files.length)return; e.preventDefault();
 Promise.all(files.map(async f=>({name:f.name,type:f.type,size:f.size,sha256:Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',await f.arrayBuffer()))).map(v=>v.toString(16).padStart(2,'0')).join('')}))).then(v=>{pasted=v;report()})
},true);
document.addEventListener('keydown',e=>{lastKey=e.key;report()},true);
document.querySelector('button').onclick=()=>{clicks++;report()};
function report(){const r=entry.getBoundingClientRect();fetch('/state',{method:'POST',body:JSON.stringify({
 documentId,value:entry.value,focused:document.activeElement.id,page:location.search,clicks,pasted,scale:devicePixelRatio,lastKey,hasFocus:document.hasFocus(),
 x:mozInnerScreenX-screenX+r.x+r.width/2,y:mozInnerScreenY-screenY+r.y+r.height/2})}).catch(()=>{})}
for(const kind of ['input','focus','blur','keyup'])document.addEventListener(kind,report,true);
setInterval(report,100);report();
</script>'''


class PageServer(BaseHTTPRequestHandler):
    def do_GET(self):
        request_paths.append(self.path)
        if self.path == '/favicon.ico':
            self.send_response(204)
            self.end_headers()
            return
        self.send_response(200)
        self.send_header('Content-Type', 'text/html; charset=utf-8')
        self.send_header('Content-Length', str(len(PAGE)))
        self.end_headers()
        self.wfile.write(PAGE)

    def do_POST(self):
        global state
        size = int(self.headers.get('Content-Length', 0))
        if not 0 < size <= 32768:
            self.send_error(400)
            return
        value = json.loads(self.rfile.read(size))
        # Background tabs must not overwrite the active document's report.
        if value.get('hasFocus') or not state or value['documentId'] == state['documentId']:
            state = value
        self.send_response(204)
        self.end_headers()

    def log_message(self, *_):
        pass


class Peer:
    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(30)
        self.socket.connect(str(path))
        self.file = self.socket.makefile('rb')
        self.events = queue.Queue()
        self.errors = []
        self.frame = None
        self.h264_frames = 0
        def read():
            try:
                while line := self.file.readline(16 * 1024 * 1024 + 1):
                    event = json.loads(line)
                    if event['type'] == 'error':
                        self.errors.append(event)
                    if event['type'] != 'frame':
                        self.events.put(event)
                    elif 'png' in event:
                        self.frame = event
                    elif event.get('codec') == 'h264':
                        self.h264_frames += 1
            except OSError:
                pass
        self.thread = threading.Thread(target=read, daemon=True)
        self.thread.start()
        peers.append(self)
        self.send(type='hello')
        self.until('hello')

    def send(self, **value):
        self.socket.sendall(json.dumps(dict(value, version=1)).encode() + b'\n')

    def until(self, kind):
        while True:
            event = self.events.get(timeout=30)
            assert event['type'] != 'error', event
            if event['type'] == kind:
                return event

    def close(self):
        try:
            self.socket.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        self.thread.join(timeout=2)
        self.file.close()
        self.socket.close()


def start(argv, env, stderr):
    process = subprocess.Popen(argv, env=env, start_new_session=True, stdin=subprocess.DEVNULL,
                               stdout=subprocess.DEVNULL, stderr=stderr)
    children.append(process)
    return process


def wait(predicate, message):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        for peer in peers:
            assert not peer.errors, peer.errors
        if predicate():
            return
        time.sleep(.02)
    raise AssertionError((message, state))


server = ThreadingHTTPServer(('127.0.0.1', 0), PageServer)
threading.Thread(target=server.serve_forever, daemon=True).start()
# Snap can read its own common directory. No default browser profile is opened.
parent = Path.home()/'snap/firefox/common' if Path('/snap/firefox/current').exists() else Path('/tmp')
try:
    with tempfile.TemporaryDirectory(prefix='rat-', dir=parent) as directory:
        root = Path(directory)
        root.chmod(0o700)
        profile = root/'profile'
        profile.mkdir(mode=0o700)
        (profile/'user.js').write_text('user_pref("browser.shell.checkDefaultBrowser",false);\n'
            'user_pref("browser.aboutwelcome.enabled",false);\n'
            'user_pref("browser.startup.homepage_override.mstone","ignore");\n'
            'user_pref("layout.css.devPixelsPerPx","1.0");\n'
            'user_pref("browser.newtabpage.enabled",false);\n'
            'user_pref("datareporting.policy.dataSubmissionEnabled",false);\n')
        home = root
        config = home/'.config/lan-remoteapp'
        config.mkdir(parents=True)
        url = f'http://127.0.0.1:{server.server_port}/'
        app_argv = ['DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/'+str(os.getuid())+'/bus',
                    '/usr/bin/systemd-run', '--user', '--scope', '--collect', '--quiet', '--unit='+scope,
                    '/usr/bin/firefox', '--no-remote', '--new-instance', '--profile', str(profile), url]
        (config/'apps.json').write_text(json.dumps([dict(id='browser-test', name='Browser test',
                                                       executable='/usr/bin/env', argv=app_argv)]))
        # The Agent must override a desktop IM module for its owned app only.
        env = dict(os.environ, GTK_IM_MODULE='fcitx', XMODIFIERS='@im=fcitx')
        for name in ('DISPLAY', 'XAUTHORITY', 'WAYLAND_DISPLAY', 'SESSION_MANAGER', 'DBUS_SESSION_BUS_ADDRESS'):
            env.pop(name, None)
        log_path = root/'process.log'
        with log_path.open('w+') as log:
            try:
                start([str(args.agent.resolve()), 'serve'], dict(env, HOME=str(home)), log)
                sock = home/'.local/state/lan-remoteapp/agent.sock'
                wait(sock.exists, 'private Agent ready')
                peer = Peer(sock)
                if not args.artifacts:
                    peer.send(type='video.enable', codec='h264')
                    peer.until('video.ready')
                peer.send(type='app.launch', app_id='browser-test', request_id='browser-regression', backend='native')
                started = peer.until('session.started')
                session = started['session_id']
                window = peer.until('window.created')
                wait(lambda: state.get('focused') == 'entry', 'private Firefox page ready')
                print('Owned Firefox ready on private display', flush=True)
                browser_envs = []
                for proc in Path('/proc').iterdir():
                    if not proc.name.isdigit():
                        continue
                    try:
                        if str(profile).encode()+b'\0' in (proc/'cmdline').read_bytes() and (proc/'exe').resolve().name == 'firefox':
                            browser_envs.append((proc/'environ').read_bytes().split(b'\0'))
                    except (OSError, PermissionError):
                        pass
                assert browser_envs and all(b'GTK_IM_MODULE=gtk-im-context-simple' in value and b'XMODIFIERS=@im=none' in value
                                            for value in browser_envs), 'owned Firefox must use controller composition'
                wid = window['window_id']

                def shortcut(letter):
                    for symbol, down in [(0xffe3, True), (ord(letter), True), (ord(letter), False), (0xffe3, False)]:
                        peer.send(type='input.key', window_id=wid, keysym=symbol, pressed=down)

                def enter():
                    for down in (True, False):
                        peer.send(type='input.key', window_id=wid, keysym=0xff0d, pressed=down)

                def focus_entry():
                    for down in (True, False):
                        peer.send(type='input.pointer', window_id=wid, x=round(state['x']*state['scale']),
                                  y=round(state['y']*state['scale']), button=1, pressed=down)
                    wait(lambda: state.get('focused') == 'entry' and state.get('hasFocus'), 'page text focus')
                    time.sleep(.15)

                wait(lambda: state.get('hasFocus'), 'native browser focus ready')
                for down in (True, False):
                    peer.send(type='input.key', window_id=wid, keysym=ord('x'), pressed=down)
                    time.sleep(.02)
                wait(lambda: state.get('value') == 'x', 'physical letter sanity check')
                print('Firefox physical input ready; testing committed text', flush=True)
                for iteration in range(8):
                    focus_entry()
                    shortcut('a')
                    expected = f'候选提交中文🙂{iteration}'
                    peer.send(type='input.text.commit', window_id=wid, text=expected)
                    wait(lambda: state.get('value') == expected, 'browser committed text')
                    shortcut('a')
                    peer.send(type='clipboard.copy', window_id=wid, request_id=f'copy-{iteration}')
                    assert peer.until('clipboard.data')['text'] == expected
                    peer.send(type='clipboard.cut', window_id=wid, request_id=f'cut-{iteration}')
                    cut = peer.until('clipboard.data')
                    assert cut['request_id'] == f'cut-{iteration}' and cut['text'] == expected
                    wait(lambda: state.get('value') == '', 'browser native cut')
                    expected = f'控制端主动粘贴🙂{iteration}'
                    peer.send(type='clipboard.paste', window_id=wid, text=expected)
                    wait(lambda: state.get('value') == expected, 'browser explicit paste')
                    shortcut('l')
                    peer.send(type='input.text.commit', window_id=wid, text=url+f'?page={iteration}')
                    enter()
                    wait(lambda: state.get('page') == f'?page={iteration}' and state.get('value') == '', 'browser navigation')
                    print(f'Navigation/copy/cut/paste cycle {iteration+1}/8 passed', flush=True)
                # Real web paste receives image/file objects, not textual paths.
                import hashlib, struct, zlib
                def png_chunk(kind, data):
                    return struct.pack('>I', len(data))+kind+data+struct.pack('>I', zlib.crc32(kind+data))
                pixels=b''.join(b'\0'+secrets.token_bytes(513*3) for _ in range(257))
                image=b'\x89PNG\r\n\x1a\n'+png_chunk(b'IHDR', struct.pack('>IIBBBBB',513,257,8,2,0,0,0))+png_chunk(b'IDAT',zlib.compress(pixels))+png_chunk(b'IEND',b'')
                def upload_clipboard(kind, entries, data):
                    stream=json.dumps(dict(kind=kind,entries=entries)).encode()+b'\n'+data
                    response=subprocess.run([str(args.agent.resolve()),'clipboard-upload'],input=stream,env=dict(env,HOME=str(home)),stdout=subprocess.PIPE,check=True,timeout=30)
                    return response.stdout.decode().strip()
                focus_entry()
                transfer=upload_clipboard('image',[dict(path='image.png',size=len(image),directory=False)],image)
                peer.send(type='clipboard.paste.bundle',window_id=wid,transfer_id=transfer)
                wait(lambda: state.get('pasted') and state['pasted'][0]['sha256']==hashlib.sha256(image).hexdigest(),'browser PNG paste as actual image')
                print('Firefox PNG paste exposed the original image file to the web page',flush=True)
                content=b'remoteapp-file-paste\0'+secrets.token_bytes(150000)
                transfer=upload_clipboard('files',[dict(path='report.txt',size=len(content),directory=False)],content)
                peer.send(type='clipboard.paste.bundle',window_id=wid,transfer_id=transfer)
                wait(lambda: state.get('pasted') and state['pasted'][0]['sha256']==hashlib.sha256(content).hexdigest(),'browser file paste as actual file')
                print('Firefox native file paste exposed the complete file to the web page',flush=True)
                shortcut('t')
                peer.send(type='input.text.commit', window_id=wid, text=url+'?tab=temporary')
                enter()
                wait(lambda: state.get('page') == '?tab=temporary', 'new test tab loaded')
                focus_entry()
                peer.send(type='input.text.commit', window_id=wid, text='新标签中文🙂')
                wait(lambda: state.get('value') == '新标签中文🙂', 'Chinese input in new tab')
                shortcut('w')
                wait(lambda: state.get('page') == '?page=7' and state.get('value') == '', 'previous tab restored')
                focus_entry()
                prefix = '关闭标签后中文🙂'
                peer.send(type='input.text.commit', window_id=wid, text=prefix)
                wait(lambda: state.get('value') == prefix, 'Chinese input after closing a tab')
                for _ in range(40):
                    peer.send(type='input.text.commit', window_id=wid, text='中🙂')
                expected = prefix+'中🙂'*40
                wait(lambda: state.get('value') == expected, '40 ordered browser commits')
                if not args.artifacts:
                    wait(lambda: peer.h264_frames > 1, 'actual H.264 frames received')
                peer.send(type='session.detach')
                peer.close()
                peer = Peer(sock)
                if not args.artifacts:
                    peer.send(type='video.enable', codec='h264')
                    peer.until('video.ready')
                peer.send(type='session.resume', session_id=session)
                peer.until('session.snapshot')
                peer.send(type='input.text.commit', window_id=wid, text='重连输入')
                wait(lambda: state.get('value') == expected+'重连输入', 'browser text after detach/resume')
                if not args.artifacts:
                    wait(lambda: peer.h264_frames > 0, 'H.264 resumes after reconnect')
                media = 'PNG diagnostic' if args.artifacts else 'H.264'
                print(f'PASS: Firefox {media}, 8 navigation/copy/cut/paste cycles, new/closed tab input, 40 Unicode commits, detach/resume and controller-only composition')
            except BaseException:
                if args.artifacts:
                    args.artifacts.mkdir(mode=0o700, parents=True, exist_ok=True)
                    (args.artifacts/'state.json').write_text(json.dumps(state))
                    for index, peer in enumerate(peers):
                        if peer.frame:
                            (args.artifacts/f'frame-{index}.png').write_bytes(base64.b64decode(peer.frame['png']))
                log.flush()
                print('Own server requests:', request_paths, flush=True)
                print(log_path.read_text()[-4000:])
                raise
            finally:
                for peer in peers:
                    peer.close()
                subprocess.run(['systemctl', '--user', 'stop', scope], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                for child in reversed(children):
                    if child.poll() is None:
                        os.killpg(child.pid, signal.SIGTERM)
                        try:
                            child.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            os.killpg(child.pid, signal.SIGKILL)
                            child.wait(timeout=5)
finally:
    server.shutdown()
    server.server_close()
