// Run with `npm run dev`, open /tests/receiver-smoke.html in a WebCodecs-capable
// browser. This uses the production receiver with mocked IPC, real H.264 decode
// and DOM input events. It does not claim to test QUIC or a remote desktop.
import {mockIPC,mockWindows} from '@tauri-apps/api/mocks';
import fixture from '../../../tests/fixtures/native-video.json';
let generation=0,index=0,sendCount=0;
const sent:{epoch:number;body:Record<string,unknown>}[]=[];
let waiting:((value:Record<string,unknown>)=>void)|undefined;
mockWindows('receiver-fixture');
mockIPC((command,args)=>{
  const value=args as Record<string,unknown>;
  if(command==='receiver_open') {generation++;index=0;return {epoch:generation,capabilities:['native.h264','input.key.logical','video.qos'],max_fps:30,selected_fps:30,snapshot:{windows:[{window_id:1,width:640,height:480,geometry_revision:1,title:'Windows H.264 接收验证',visible:true}]}};}
  if(command==='receiver_next') {
    if(index<fixture.frames.length){const n=index++;return {type:'frame',window_id:1,geometry_revision:1,codec:'h264',width:640,height:480,encoded_width:640,encoded_height:480,stream_id:1,key_frame:n===0,data:fixture.frames[n],frame_id:n+1};}
    return new Promise<Record<string,unknown>>(resolve=>{waiting=resolve;});
  }
  if(command==='receiver_send'){sent.push(value as typeof sent[number]);sendCount++;}
  if(command==='receiver_disconnect'){waiting?.({type:'session.exited'});waiting=undefined;}
});
const pause=(ms:number)=>new Promise(r=>setTimeout(r,ms));
function assert(value:unknown,message:string){if(!value)throw Error(message);}
async function until(check:()=>boolean){for(let i=0;i<200;i++){if(check())return;await pause(25);}throw Error('timeout');}
try {
  await import('../src/receiver');
  await until(()=>document.querySelector<HTMLDivElement>('#empty')!.hidden);
  const canvas=document.querySelector('canvas')!;
  const pixel=canvas.getContext('2d')!.getImageData(320,240,1,1).data;
  assert(pixel[0]>200&&pixel[1]<30&&pixel[2]<30,'decoded frame must be red');
  const input=document.querySelector<HTMLTextAreaElement>('#ime')!;
  input.dispatchEvent(new CompositionEvent('compositionstart'));
  input.value='中文输入🙂';input.dispatchEvent(new InputEvent('input',{isComposing:true}));
  assert(!sent.some(s=>s.body.type==='input.text.commit'),'composition must stay local');
  input.dispatchEvent(new CompositionEvent('compositionend',{data:input.value}));
  input.dispatchEvent(new InputEvent('input'));
  await until(()=>sent.some(s=>s.body.type==='input.text.commit'));
  assert(sent.filter(s=>s.body.type==='input.text.commit').length===1,'composition committed twice');
  assert(sent.find(s=>s.body.type==='input.text.commit')!.body.text==='中文输入🙂','text changed');
  input.dispatchEvent(new KeyboardEvent('keydown',{key:'Control',code:'ControlLeft',ctrlKey:true,cancelable:true}));
  input.dispatchEvent(new KeyboardEvent('keydown',{key:'c',code:'KeyC',ctrlKey:true,cancelable:true}));
  window.dispatchEvent(new Event('blur'));
  await until(()=>sent.some(s=>s.body.key==='Control_L'&&s.body.pressed===false));
  assert(sent.some(s=>s.body.key==='c'&&s.body.pressed===false),'blur must release shortcut');
  const oldCount=sent.length;
  document.querySelector<HTMLButtonElement>('#retry')!.click();
  input.value='离线文字';input.dispatchEvent(new InputEvent('input'));
  await until(()=>generation===2&&document.querySelector<HTMLDivElement>('#empty')!.hidden);
  assert(!sent.slice(oldCount).some(s=>s.body.text==='离线文字'),'offline input replayed');
  const result=document.createElement('output');result.id='smoke-result';result.textContent=`PASS · H.264 像素、中文一次提交、按键释放、重连输入隔离 · ${sendCount} controls`;
  result.style.cssText='position:fixed;bottom:45px;left:20px;padding:12px;background:#14532d;color:white;z-index:10';document.body.append(result);
}catch(error){const result=document.createElement('output');result.id='smoke-result';result.textContent=`FAIL · ${String(error)}`;result.style.cssText='position:fixed;bottom:45px;left:20px;padding:12px;background:#7f1d1d;color:white;z-index:10';document.body.append(result);}
