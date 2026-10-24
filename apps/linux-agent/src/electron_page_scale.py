"""Private ChatGPT page-scale lease. Never accepts a URL or JavaScript from a peer.

The parent owns window resizing/capture. This process owns CSS and an X11
snapshot, and restores both on stdin EOF (including publisher termination).
"""
import ctypes as C
import json
import math
import os
from pathlib import Path
import pwd
import select
import signal
import subprocess
import sys
import time
import urllib.request
import uuid

APP = '/usr/lib/chatgpt/ChatGPT'
MAX_REPLY = 1024 * 1024


def identity(pid):
    p = Path('/proc') / str(pid)
    if p.stat().st_uid != os.getuid() or str((p / 'exe').resolve()) != APP:
        raise ValueError('page_scale_unsupported')
    return int((p / 'stat').read_text().rsplit(')', 1)[1].split()[19])


def endpoint(pid):
    # Only the current user's actual ChatGPT profile, never a peer-provided path.
    port = int((Path(pwd.getpwuid(os.getuid()).pw_dir) / '.config/Codex/DevToolsActivePort').read_text().splitlines()[0])
    sockets = set()
    for fd in (Path('/proc') / str(pid) / 'fd').iterdir():
        try:
            link = os.readlink(fd)
            if link.startswith('socket:['):
                sockets.add(link[8:-1])
        except FileNotFoundError:
            pass
    listeners = []
    for row in (Path('/proc') / str(pid) / 'net/tcp').read_text().splitlines()[1:]:
        cols = row.split()
        if cols[3] == '0A' and cols[9] in sockets and int(cols[1].split(':')[1], 16) == port:
            listeners.append(cols[1].split(':')[0])
    # A second IPv6/wildcard listener on the same port would expose CDP.
    for row in (Path('/proc') / str(pid) / 'net/tcp6').read_text().splitlines()[1:]:
        cols = row.split()
        if cols[3] == '0A' and cols[9] in sockets and int(cols[1].split(':')[1], 16) == port:
            listeners.append(cols[1].split(':')[0])
    if listeners != ['0100007F']:
        raise ValueError('page_scale_restart_required')
    return port


class CDP:
    def __init__(self, port, target):
        import websocket
        if not target or any(c not in '0123456789abcdefABCDEF' for c in target):
            raise ValueError('page_scale_target_ambiguous')
        self.ws = websocket.create_connection(f'ws://127.0.0.1:{port}/devtools/page/{target}',
                                             timeout=2, suppress_origin=True, http_proxy_host=None)
        self.seq = 0

    def call(self, method, params=None):
        self.seq += 1
        self.ws.send(json.dumps(dict(id=self.seq, method=method, params=params or {})))
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            raw = self.ws.recv()
            if len(raw) > MAX_REPLY:
                raise ValueError('page_scale_unavailable')
            reply = json.loads(raw)
            if reply.get('id') == self.seq:
                if 'error' in reply:
                    raise ValueError('page_scale_unavailable')
                return reply.get('result', {})
        raise TimeoutError('page_scale_unavailable')

    def js(self, expression):
        r = self.call('Runtime.evaluate', dict(expression=expression, returnByValue=True))
        if 'exceptionDetails' in r:
            raise ValueError('page_scale_unavailable')
        return r['result'].get('value')

    def close(self):
        self.ws.close()


class Data(C.Union):
    _fields_ = [('b', C.c_char * 20), ('s', C.c_short * 10), ('l', C.c_long * 5)]


class Message(C.Structure):
    _fields_ = [('type', C.c_int), ('serial', C.c_ulong), ('send_event', C.c_int),
                ('display', C.c_void_p), ('window', C.c_ulong), ('message_type', C.c_ulong),
                ('format', C.c_int), ('data', Data)]


class Event(C.Union):
    _fields_ = [('client', Message), ('pad', C.c_long * 24)]


class XWindow:
    def __init__(self, display, authority, xid, pid):
        os.environ['XAUTHORITY'] = authority
        self.x = C.CDLL('libX11.so.6')
        self.x.XOpenDisplay.argtypes = [C.c_char_p]; self.x.XOpenDisplay.restype = C.c_void_p
        self.x.XDefaultRootWindow.argtypes = [C.c_void_p]; self.x.XDefaultRootWindow.restype = C.c_ulong
        self.x.XInternAtom.argtypes = [C.c_void_p, C.c_char_p, C.c_int]; self.x.XInternAtom.restype = C.c_ulong
        self.x.XGetGeometry.argtypes = [C.c_void_p, C.c_ulong, C.POINTER(C.c_ulong), C.POINTER(C.c_int), C.POINTER(C.c_int), *([C.POINTER(C.c_uint)] * 4)]
        self.x.XTranslateCoordinates.argtypes = [C.c_void_p, C.c_ulong, C.c_ulong, C.c_int, C.c_int, C.POINTER(C.c_int), C.POINTER(C.c_int), C.POINTER(C.c_ulong)]
        self.x.XGetWindowProperty.argtypes = [C.c_void_p, C.c_ulong, C.c_ulong, C.c_long, C.c_long, C.c_int, C.c_ulong, C.POINTER(C.c_ulong), C.POINTER(C.c_int), C.POINTER(C.c_ulong), C.POINTER(C.c_ulong), C.POINTER(C.c_void_p)]
        self.x.XChangeProperty.argtypes = [C.c_void_p, C.c_ulong, C.c_ulong, C.c_ulong, C.c_int, C.c_int, C.c_void_p, C.c_int]
        self.x.XDeleteProperty.argtypes = [C.c_void_p, C.c_ulong, C.c_ulong]
        self.x.XSendEvent.argtypes = [C.c_void_p, C.c_ulong, C.c_int, C.c_long, C.POINTER(Event)]
        self.x.XSync.argtypes = [C.c_void_p, C.c_int]; self.x.XFree.argtypes = [C.c_void_p]
        self.x.XCloseDisplay.argtypes = [C.c_void_p]
        # A destroyed window is an expected lease ending, not a fatal Xlib exit.
        self.error_handler = C.CFUNCTYPE(C.c_int, C.c_void_p, C.c_void_p)(lambda *_: 0)
        self.x.XSetErrorHandler(self.error_handler)
        self.d = self.x.XOpenDisplay(display.encode())
        if not self.d:
            raise ValueError('page_scale_unsupported')
        self.root = self.x.XDefaultRootWindow(self.d)
        self.xid, self.pid = xid, pid
        self.owner = uuid.uuid4().hex.encode()
        self.saved = None

    def atom(self, name):
        return self.x.XInternAtom(self.d, name.encode(), 0)

    def prop(self, name):
        actual, fmt, count, remaining, data = C.c_ulong(), C.c_int(), C.c_ulong(), C.c_ulong(), C.c_void_p()
        status = self.x.XGetWindowProperty(self.d, self.xid, self.atom(name), 0, 1024, 0, 0,
                    C.byref(actual), C.byref(fmt), C.byref(count), C.byref(remaining), C.byref(data))
        if status or remaining.value:
            raise ValueError('page_scale_window_changed')
        try:
            if fmt.value == 8:
                return C.string_at(data, count.value)
            if fmt.value == 32:
                return list(C.cast(data, C.POINTER(C.c_ulong))[:count.value])
            return []
        finally:
            if data:
                self.x.XFree(data)

    def geometry(self):
        if self.prop('_NET_WM_PID') != [self.pid]:
            raise ValueError('page_scale_window_changed')
        root, x, y, w, h, border, depth = C.c_ulong(), C.c_int(), C.c_int(), C.c_uint(), C.c_uint(), C.c_uint(), C.c_uint()
        if not self.x.XGetGeometry(self.d, self.xid, C.byref(root), C.byref(x), C.byref(y), C.byref(w), C.byref(h), C.byref(border), C.byref(depth)):
            raise ValueError('page_scale_window_changed')
        child = C.c_ulong()
        self.x.XTranslateCoordinates(self.d, self.xid, self.root, 0, 0, C.byref(x), C.byref(y), C.byref(child))
        return [x.value, y.value, w.value, h.value]

    def claim(self):
        if self.prop('_PANEFERRY_PAGE_SCALE_OWNER'):
            raise ValueError('page_scale_busy')
        self.saved = self.geometry()
        self.states = self.prop('_NET_WM_STATE')
        self.x.XChangeProperty(self.d, self.xid, self.atom('_PANEFERRY_PAGE_SCALE_OWNER'), self.atom('STRING'), 8, 0, self.owner, len(self.owner))
        self.x.XSync(self.d, 0)

    def send(self, name, values):
        e = Event(); e.client.type = 33; e.client.display = self.d; e.client.window = self.xid
        e.client.message_type = self.atom(name); e.client.format = 32
        for i, value in enumerate(values):
            e.client.data.l[i] = value
        self.x.XSendEvent(self.d, self.root, 0, (1 << 19) | (1 << 20), C.byref(e))
        self.x.XSync(self.d, 0)

    def restore(self):
        if self.saved is None or self.prop('_PANEFERRY_PAGE_SCALE_OWNER') != self.owner:
            return
        self.geometry()  # PID/XID still belongs to our lease.
        x, y, w, h = self.saved
        states = ['_NET_WM_STATE_MAXIMIZED_HORZ', '_NET_WM_STATE_MAXIMIZED_VERT', '_NET_WM_STATE_FULLSCREEN']
        for name in states:
            self.send('_NET_WM_STATE', [0, self.atom(name), 0, 2, 0])
        self.send('_NET_MOVERESIZE_WINDOW', [(15 << 8) | (2 << 12) | 10, x, y, w, h])
        for _ in range(30):
            if self.geometry() == self.saved:
                break
            time.sleep(.05)
        for name in states:
            if self.atom(name) in self.states:
                self.send('_NET_WM_STATE', [1, self.atom(name), 0, 2, 0])
        for _ in range(30):
            if self.geometry() == self.saved:
                break
            time.sleep(.05)
        if self.geometry() != self.saved:
            raise ValueError('page_scale_restore_incomplete')
        self.x.XDeleteProperty(self.d, self.xid, self.atom('_PANEFERRY_PAGE_SCALE_OWNER'))
        self.x.XSync(self.d, 0)

    def close(self):
        self.x.XCloseDisplay(self.d)


def select_target(port, bounds):
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(f'http://127.0.0.1:{port}/json/list', timeout=2) as response:
        raw = response.read(MAX_REPLY + 1)
    if len(raw) > MAX_REPLY:
        raise ValueError('page_scale_unavailable')
    candidates = []
    for t in json.loads(raw):
        if t.get('type') != 'page' or t.get('url', '').split('?', 1)[0].split('#', 1)[0] != 'app://-/index.html':
            continue
        c = CDP(port, t['id'])
        try:
            info = c.js('({bounds:[screenX,screenY,outerWidth,outerHeight],dpr:devicePixelRatio,top:top===self,url:location.origin+location.pathname})')
            if info and info['bounds'] == bounds and info['dpr'] == 1 and info['top'] and info['url'] == 'app://-/index.html':
                candidates.append(t['id'])
        finally:
            c.close()
    if len(candidates) != 1:
        raise ValueError('page_scale_target_ambiguous')
    return candidates[0]


def installation(key):
    # This application's shell already compensates its viewport and portal
    # coordinates for --codex-window-zoom. Root CSS zoom does NOT do that.
    # Browser WebViews live in an unzoomed portal; give those elements their own
    # CSS zoom and constrain them to the portal, without changing guest state.
    return '''(()=>{
      if(top!==self || location.origin+location.pathname!=='app://-/index.html')return false;
      const key=KEY, property='--codex-window-zoom';
      if(globalThis[key])return globalThis[key].renew();
      const el=document.documentElement;
      if(!el || !document.querySelector('[style*="zoom: var(--codex-window-zoom)"]'))return false;
      const style=el.style, value=style.getPropertyValue(property), priority=style.getPropertyPriority(property);
      const base=Number(getComputedStyle(el).getPropertyValue(property));
      if(!Number.isFinite(base)||base<=0)return false;
      const target=String(base*2), sheet=document.createElement('style');
      sheet.textContent='webview.h-full.w-full {zoom:2!important;width:100%!important;height:100%!important}';
      const supported=()=>[...document.querySelectorAll('webview')].every(w=>{
        const r=w.getBoundingClientRect();
        if(r.width<=1||r.height<=1||getComputedStyle(w).visibility==='hidden')return true;
        return w.matches('.h-full.w-full') && getComputedStyle(w.parentElement).position==='fixed';
      });
      if(!supported())return false;
      const lease={until:performance.now()+4000,renew:()=>{
        if(style.getPropertyValue(property)!==target || !sheet.isConnected || !supported())return false;
        lease.until=performance.now()+4000;return true;
      },restore:()=>{
        clearInterval(lease.timer);sheet.remove();
        if(value)style.setProperty(property,value,priority);else style.removeProperty(property);
        delete globalThis[key];
      }};
      globalThis[key]=lease;
      document.head.append(sheet);style.setProperty(property,target,'important');
      lease.timer=setInterval(()=>{if(performance.now()>lease.until)lease.restore();},250);
      return true;
    })()'''.replace('KEY', json.dumps(key))


class Lease:
    def __init__(self, request):
        self.pid, self.started = request['pid'], request['started']
        if identity(self.pid) != self.started:
            raise ValueError('page_scale_window_changed')
        self.x = XWindow(request['display'], request['authority'], request['window_id'], self.pid)
        self.cdp = None; self.key = '__paneferry_page_' + uuid.uuid4().hex
        self.expression = installation(self.key)
        try:
            bounds = self.x.geometry()
            self.size = [bounds[2] * 2, bounds[3] * 2]
            if self.size[0] > 4096 or self.size[1] > 2160 or math.prod(self.size) > 8_000_000:
                raise ValueError('resolution_limited')
            try:
                port = endpoint(self.pid)
            except Exception as error:
                raise ValueError("page_scale_restart_required") from error
            self.target = select_target(port, bounds)
            self.cdp = CDP(port, self.target)
            if self.x.geometry() != bounds:
                raise ValueError('page_scale_window_changed')
            self.x.claim()
            if not self.cdp.js(self.expression):
                raise ValueError('page_scale_unavailable')
        except Exception:
            self.restore()
            raise

    def tick(self):
        if identity(self.pid) != self.started or self.x.prop('_PANEFERRY_PAGE_SCALE_OWNER') != self.x.owner:
            raise ValueError('page_scale_window_changed')
        if not self.cdp.js(self.expression):
            raise ValueError('page_scale_navigation_unsupported')

    def restore(self):
        # Each cleanup is independent: losing the renderer must not skip geometry.
        failures = []
        if self.cdp:
            try: self.cdp.js(f'globalThis[{json.dumps(self.key)}]?.restore();true')
            except Exception: failures.append('css')  # The in-page expiry is the fallback.
            self.cdp.close(); self.cdp = None
        try:
            if identity(self.pid) == self.started:
                self.x.restore()
        except Exception:
            failures.append('geometry')
        self.x.close()
        if failures:
            print('page_scale_restore_incomplete:' + ','.join(failures), file=sys.stderr, flush=True)
        return not failures


def reply(value):
    print(json.dumps(value, separators=(',', ':')), flush=True)


def serve():
    lease = None
    stopping = False
    def request_stop(*_):
        nonlocal stopping
        stopping = True
    # systemd may SIGTERM the remaining cgroup when the publisher dies.
    # Finish recovery instead of letting the default signal action skip finally.
    signal.signal(signal.SIGTERM, request_stop)
    signal.signal(signal.SIGINT, request_stop)
    # os.read avoids TextIO buffering hiding a second command from select().
    pending = b''
    try:
        while not stopping:
            ready, _, _ = select.select([sys.stdin.fileno()], [], [], .5)
            if ready:
                chunk = os.read(sys.stdin.fileno(), 65536)
                if not chunk:
                    return
                pending += chunk
                if len(pending) > 65536:
                    raise ValueError('invalid_request')
                while b'\n' in pending:
                    line, pending = pending.split(b'\n', 1)
                    r = json.loads(line)
                    if r['op'] == 'start' and lease is None:
                        lease = Lease(r)
                        reply({'size': lease.size})
                    elif r['op'] == 'commit' and lease:
                        for _ in range(40):
                            if lease.x.geometry()[2:] == lease.size:
                                break
                            lease.tick(); time.sleep(.05)
                        if lease.x.geometry()[2:] != lease.size:
                            raise ValueError('page_scale_resize_failed')
                        reply({'active': True})
                    elif r['op'] == 'stop' and lease:
                        ok = lease.restore(); lease = None
                        reply({'restored': ok}); return
                    else:
                        raise ValueError('invalid_request')
            if lease:
                lease.tick()
    except Exception as error:
        code = str(error) if str(error).startswith(('page_scale_', 'resolution_limited')) else 'page_scale_unavailable'
        reply({'error': code})
    finally:
        if lease:
            lease.restore()


def prepare(pid):
    import websocket  # Verify the optional dependency before stopping an app.
    started = identity(pid)
    try:
        port = endpoint(pid)
        reply({'pid': pid, 'prepared': True}); return
    except Exception:
        pass
    # A local, explicit command only. No remote protocol can restart an app.
    env = dict(os.environ)
    result = subprocess.run(['systemctl', '--user', 'show-environment'], capture_output=True, text=True, check=True)
    env.update(line.split('=', 1) for line in result.stdout.splitlines() if '=' in line)
    if not env.get('DISPLAY') or not env.get('XAUTHORITY'):
        raise ValueError('page_scale_restart_required')
    if identity(pid) != started:
        raise ValueError('page_scale_window_changed')
    argv = (Path('/proc') / str(pid) / 'cmdline').read_bytes().split(b'\0')[:-1]
    if argv != [APP.encode()]:
        raise ValueError('page_scale_custom_launch_unsupported')
    os.kill(pid, signal.SIGTERM)
    for _ in range(150):
        if not Path(f'/proc/{pid}').exists():
            break
        time.sleep(.2)
    else:
        raise ValueError('page_scale_restart_timeout')
    # Keep the real profile and desktop session; no copied credentials or new profile.
    subprocess.run(['systemd-run', '--user', '--collect', '--unit=paneferry-chatgpt-debug',
                    '--working-directory=' + pwd.getpwuid(os.getuid()).pw_dir,
                    APP, '--remote-debugging-address=127.0.0.1', '--remote-debugging-port=0'],
                   env=env, check=True, stdout=subprocess.DEVNULL)
    for _ in range(100):
        result = subprocess.run(['systemctl', '--user', 'show', 'paneferry-chatgpt-debug', '-p', 'MainPID', '--value'],
                                capture_output=True, text=True, check=True)
        try:
            new_pid = int(result.stdout.strip())
            identity(new_pid); endpoint(new_pid)
            reply({'pid': new_pid, 'restarted': True, 'prepared': True}); return
        except (OSError, ValueError, IndexError):
            time.sleep(.2)
    raise ValueError('page_scale_restart_required')


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] == '--prepare':
        prepare(int(sys.argv[2]))
    else:
        serve()
