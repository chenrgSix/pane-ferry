import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {metadata,pointer,resizePixels,sourceSize,keysym,logicalKey,annexB,spsInfo,inspectFrame} from '../src/receiver-core.ts';
const fixture=JSON.parse(fs.readFileSync(new URL('../../../tests/fixtures/native-video.json',import.meta.url)));
const window=metadata({window_id:1,width:640,height:480,geometry_revision:1,title:'测试'});
function frame(index=0){return {window_id:1,codec:'h264',width:640,height:480,encoded_width:640,encoded_height:480,stream_id:1,key_frame:index===0,geometry_revision:1,data:fixture.frames[index]};}
test('letterbox bars never click a remote window; drags clamp to source edges',()=>{
  assert.equal(pointer(1,250,1000,500,640,480),null);
  assert.deepEqual(pointer(500,250,1000,500,640,480),{x:320,y:240});
  assert.deepEqual(pointer(-30,700,1000,500,640,480,true),{x:0,y:479});
  assert.equal(pointer(0,0,0,500,640,480),null);
});
test('logical resize preserves aspect ratio under protocol pixel budget',()=>{
  assert.deepEqual(resizePixels(1280,720),{width:1280,height:720});
  const scaled=metadata({window_id:1,width:1280,height:960,geometry_revision:1,scale_factor:2});
  assert.deepEqual(resizePixels(640,480,scaled.scale_factor),{width:1280,height:960});
  assert.throws(()=>metadata({...scaled,scale_factor:3}));
  const size=resizePixels(7680,4320);assert.ok(size.width<=4096&&size.height<=2160&&size.width*size.height<=8_000_000);
  assert.ok(Math.abs(size.width/size.height-16/9)<.002);
});
test('original picture follows source dimensions and density without local DPI',()=>{
  assert.deepEqual(sourceSize(window),{width:640,height:480});
  assert.deepEqual(sourceSize(metadata({...window,width:1600,height:1200,scale_factor:2})),{width:800,height:600});
});
test('real Annex B fixture begins with validated SPS, PPS and IDR',()=>{
  const info=spsInfo(annexB(Uint8Array.from(Buffer.from(fixture.frames[0],'base64'))).find(n=>(n[0]&31)===7));
  assert.deepEqual(info,{width:640,height:480,codec:'avc1.42c01e'});
  assert.equal(inspectFrame(frame(),window,true).key,true);
  for(let i=1;i<fixture.frames.length;i++)assert.equal(inspectFrame(frame(i),window,false).key,false);
});
test('reconnect cannot start on dependent frames or accept changed geometry',()=>{
  assert.throws(()=>inspectFrame(frame(1),window,true),/missing_keyframe/);
  assert.equal(inspectFrame({...frame(),geometry_revision:2},window,true),null);
  assert.throws(()=>inspectFrame({...frame(),encoded_width:641},window,true));
  assert.throws(()=>inspectFrame({...frame(),width:320},window,true));
  assert.throws(()=>inspectFrame({...frame(),data:'AAAAAWf/////'},window,true));
  assert.throws(()=>inspectFrame({...frame(),stream_id:0},window,true));
  assert.throws(()=>metadata({window_id:1,width:8192,height:480,geometry_revision:1}));
});
test('navigation, right modifiers and shortcuts use the existing keysym wire format',()=>{
  assert.equal(keysym('Control','ControlRight'),0xffe4);
  assert.equal(keysym('ArrowLeft','ArrowLeft'),0xff51);
  assert.equal(keysym('F12','F12'),0xffc9);
  assert.equal(keysym('c','KeyC'),99);
  assert.equal(keysym('Process',''),null);
});
test('Mac logical input uses GTK names and preserves shortcut physical keys',()=>{
  assert.equal(logicalKey('Control','ControlRight'),'Control_R');
  assert.equal(logicalKey('Enter','Enter'),'Return');
  assert.equal(logicalKey('ArrowLeft','ArrowLeft'),'Left');
  assert.equal(logicalKey('!','Digit1'),'1');
  assert.equal(logicalKey(':','Semicolon'),'semicolon');
  assert.equal(logicalKey('Process',''),null);
});
