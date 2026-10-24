// Exercise fail-closed scene ownership with delayed/failed compositor teardown.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const code = readFileSync(new URL('../../apps/linux-agent/gnome-extension/applicationScene.js', import.meta.url), 'utf8')
  .replace(/^import .*;\n/gm, '').replace(/^export /gm, '');
class Actor {
  constructor(properties = {}) { Object.assign(this, properties); this.children = []; this.visible = true; this.width = 100; this.height = 100; }
  add_child(child) { this.children.push(child); child.parent = this; this.added?.(); }
  get_parent() { return this.parent; }
  get_children() { return this.children; }
  get_last_child() { return this.children.at(-1); }
  set_child_above_sibling(child) { this.children = this.children.filter(c => c !== child); this.children.push(child); }
  set_position(x,y) { this.position = [x,y]; }
  set_size(width,height) { this.width=width;this.height=height; }
  hide() { this.visible=false; } show() { this.visible=true; }
  destroy() { this.destroyed=true; }
  connect(name, callback) { this.handlers ??= new Map(); const id=this.handlers.size+1; this.handlers.set(id,callback); if(name==='child-added')this.added=callback; return id; }
  disconnect(id) { this.handlers.delete(id); }
}
function fixture() {
  const root = {get_buffer_rect:()=>({x:0,y:0,width:100,height:100}), minimized:false};
  const actor = new Actor({meta_window:root}); root.get_compositor_private=()=>actor;
  const stranger = {get_buffer_rect:()=>({x:0,y:0,width:100,height:100}), minimized:false};
  const foreign = new Actor({meta_window:stranger});
  const stage = new Actor();stage.get_actor_at_pos=()=>actor;
  let visibleWindows=[root], allActors=[actor], monitor=0, fail=false;
  const output = {connector:'Meta-1',scale:1,manager:{get_monitor_for_connector:()=>monitor}, requireRoot:()=>root,close:async()=>{}};
  const timers=[], calls=[];
  const Gio={DBusCallFlags:{NO_AUTO_START:0},DBusSignalFlags:{NONE:0},DBus:{session:{
    call(_b,_p,_i,method,_params,_rt,_flags,_timeout,_cancel,callback){
      calls.push({method,covered:stage.get_last_child()?.background_color?.alpha===255});
      queueMicrotask(()=>callback({call_finish:()=>({deep_unpack:()=>[method==='CreateSession'?'/session':'/stream']})},{}));
    }, signal_subscribe:()=>1,signal_unsubscribe:()=>{},
  }}};
  const GLib={Variant:class{},PRIORITY_DEFAULT:0,SOURCE_CONTINUE:true,SOURCE_REMOVE:false,timeout_add:(_p,_ms,callback)=>timers.push(callback)};
  const Main={sessionMode:{isLocked:false},overview:{hide(){}},activateWindow(){}};
  const global={stage,get_window_actors:()=>allActors,display:{get_monitor_geometry:()=>({x:0,y:0,width:100,height:100}),sort_windows_by_stacking:scope=>scope,focus_window:root}};
  const Scene=vm.runInNewContext(code+'\nApplicationScene',{Clutter:{Actor,Clone:Actor,PickMode:{REACTIVE:1}},Cogl:{Color:class{constructor(p){Object.assign(this,p)}}},Gio,GLib,Main,global});
  const scene=new Scene(output,()=>{if(fail)throw Error('gone');return visibleWindows;});
  return {scene,stage,root,foreign,Main,calls,timers, scope:(v)=>visibleWindows=v,actors:(v)=>allActors=v,
    fail:()=>fail=true,removeMonitor:()=>monitor=-1};
}
{
  const f=fixture(); await f.scene.start();
  assert.ok(f.calls.every(c=>c.covered),'all recording calls occur after opaque cover');
  f.stage.add_child(new Actor()); assert.equal(f.stage.get_last_child(),f.scene.cover);
  assert.equal(f.scene.clones.size,1);
  f.Main.sessionMode.isLocked=true;f.scene.update();assert.equal(f.scene.clones.size,0);
  assert.equal(f.scene.cover.visible,true);assert.equal(f.scene.cover.background_color.alpha,255);
  f.Main.sessionMode.isLocked=false;f.scene.update();assert.equal(f.scene.clones.size,1);
  f.fail();f.scene.update();assert.ok([...f.scene.clones.values()].every(c=>!c.visible));
  await f.scene.close();assert.notEqual(f.scene.cover.destroyed,true,'failed monitor removal keeps opaque cover');
  f.removeMonitor();f.timers[0]();assert.equal(f.scene.cover.destroyed,true);
}
{
  const f=fixture();f.actors([f.foreign]);
  assert.throws(()=>f.scene.point(20,20,100,100),/window_unavailable/,'foreign actor overlapping a grab cannot receive hidden clicks');
  f.actors([]);assert.deepEqual({...f.scene.point(20,20,100,100)},{x:20,y:20});
  assert.equal(f.scene.cover.visible,true);
  assert.throws(()=>f.scene.point(-1,20,100,100),/window_unavailable/);
  f.removeMonitor();await f.scene.close();
}
console.log('PASS: opaque-before-recording, stage stacking, lock/source loss, fail-closed teardown and foreign input exclusion');
