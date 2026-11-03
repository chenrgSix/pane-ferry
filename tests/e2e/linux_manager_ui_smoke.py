#!/usr/bin/env python3
"""Actual GTK settings widgets/persistence in a private application bus/HOME."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

if '--private' not in sys.argv:
    with tempfile.TemporaryDirectory(prefix='pf-ui-') as home:
        env=dict(os.environ,HOME=home)
        conf=Path(home)/'bus.conf'
        conf.write_text('<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen><policy context="default"><allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/></policy></busconfig>')
        raise SystemExit(subprocess.call(['dbus-run-session','--config-file='+str(conf),'--',sys.executable,__file__,'--private'],env=env))
root=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(root/'apps/linux-client'))
import main
from gi.repository import Gtk
class Audio:
    def __init__(self,changed):self.subscriptions={};self.changed=changed
    def set(self,peer,enabled):
        if enabled:self.subscriptions[peer]=True
        else:self.subscriptions.pop(peer,None)
    def stop(self):self.subscriptions.clear()
main.audio.Service=Audio
main.Manager.initialize=lambda *_:None
main.Manager.load=lambda *_:None
manager=main.Manager();manager.register(None);manager.activate()
peer='quic:00000000-0000-4000-8000-000000000001'
main.peers=lambda:[dict(device_id=peer[5:],connection_id=peer,name='Fixture',address='192.168.8.20:42424',generation=peer[5:])]
manager.selector.append(peer,'Fixture');manager.selector.set_active_id(peer)
manager.connection_name.set_text('办公电脑');manager.connection_address.set_text('10.0.0.20:42424')
manager.save_connection()
assert manager.saved_selector.get_active_id()==peer[5:]
assert manager.saved_connections.entries==[dict(peerId=peer[5:],name='办公电脑',address='10.0.0.20:42424')]
manager.full_color.set_active(True);manager.source_option.set_active(True);manager.scene_option.set_active(True)
manager.scale_option.set_active_id('2');manager.takeover_option.set_active(True);manager.audio_option.set_active(True)
assert manager.video_color=='444-atlas-v2' and manager.source_size and manager.application_scene and manager.render_scale==2
assert not manager.scene_option.get_sensitive()
assert peer in manager.takeover and peer in manager.audio_enabled and peer in manager.audio.subscriptions
loaded=main.Manager()
assert loaded.saved_connections.entries==manager.saved_connections.entries
manager.remove_saved()
assert not manager.saved_connections.entries and manager.selected==peer
assert loaded.video_color==manager.video_color and loaded.render_scale==2 and loaded.source_size
assert loaded.takeover==manager.takeover and loaded.audio_enabled==manager.audio_enabled
assert not loaded.audio.subscriptions,'loading preferences must not immediately subscribe to an unpaired peer'
manager.disconnect_device()
assert peer not in manager.takeover and peer in manager.audio_enabled,'disconnecting video must preserve per-device audio'
operations=[];manager.network_job=lambda op,**kw:operations.append((op,kw))
manager.revoke_device()
assert peer not in manager.audio_enabled and not manager.audio.subscriptions and operations[-1][0]=='revoke'
manager.window.destroy();manager.audio.stop();loaded.audio.stop()
while Gtk.events_pending():Gtk.main_iteration_do(False)
print('PASS actual GTK: color/source/scene/render-scale controls, per-device audio/takeover persistence, video-only disconnect and revoke cleanup')
