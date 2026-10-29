import {invoke} from '@tauri-apps/api/core';
import {getCurrentWindow} from '@tauri-apps/api/window';
import {inspectFrame,keysym,logicalKey,metadata,pointer,resizePixels,type RemoteWindow} from './receiver-core';
import './receiver.css';

const root=document.querySelector<HTMLDivElement>('#app')!;
root.innerHTML=`<header><strong>窗渡 PaneFerry</strong><select id="members" aria-label="远端窗口"></select><label>显示 <select id="size"><option value="auto">自动适应窗口</option><option value="source">保留远端尺寸</option><option value="1280x720">1280 × 720</option><option value="1920x1080">1920 × 1080</option></select></label><label>帧率 <select id="fps"><option value="30">30</option><option value="60">60</option></select></label><button id="retry">重新连接</button><button id="close">断开</button></header><main id="view"><canvas></canvas><textarea id="ime" aria-label="远程输入" autocapitalize="off" autocomplete="off" spellcheck="false"></textarea><div id="empty">正在连接远程应用…</div></main><footer><span id="status" role="status">正在连接…</span><span id="rate"></span><span>关闭窗口仅断开连接，远端应用继续运行</span></footer>`;
const element=<T extends HTMLElement>(id:string)=>document.getElementById(id) as T;
const view=element<HTMLDivElement>('view'),canvas=view.querySelector('canvas')!,context=canvas.getContext('2d')!;
const ime=element<HTMLTextAreaElement>('ime'),members=element<HTMLSelectElement>('members'),size=element<HTMLSelectElement>('size'),fps=element<HTMLSelectElement>('fps');
const status=element<HTMLSpanElement>('status'),empty=element<HTMLDivElement>('empty');
const local=getCurrentWindow();
let epoch=0,run=0,ready=false,selected=0,composing=false,closing=false,retryTimer:ReturnType<typeof setTimeout>|undefined;
let capabilities:string[]=[],catalog=new Map<number,RemoteWindow>();
let displayed:RemoteWindow|undefined;
const waitingKey=new Set<number>();
let sentResize='',lastPoint={x:0,y:0},painted=0,rateStart=performance.now();
const pressed=new Map<string,Record<string,unknown>>(),buttons=new Set<number>();
type Video={decoder:VideoDecoder;stream:number;revision:number;pending:Map<number,number|undefined>;stamp:number};
const videos=new Map<number,Video>();
let stamp=0,clock=0;
const outgoing:{epoch:number;body:Record<string,unknown>}[]=[];
let sending=false;
function message(text:string){status.textContent=text;}
function clearVideo(id?:number) {
  for(const [wid,video] of videos)if(id===undefined||wid===id){if(video.decoder.state!=='closed')video.decoder.close();videos.delete(wid);}
}
function invalidate(text:string) {
  ready=false;outgoing.length=0;pressed.clear();buttons.clear();ime.value='';composing=false;displayed=undefined;waitingKey.clear();
  clearVideo();context.clearRect(0,0,canvas.width,canvas.height);empty.hidden=false;empty.textContent=text;message(text);
}
function transmit(body:Record<string,unknown>,windowId=selected) {
  if(!ready)return;
  if(!['video.feedback'].includes(String(body.type)))body.window_id=windowId;
  if(outgoing.length>=128){void fail('输入队列已满，正在重新连接。');return;}
  outgoing.push({epoch,body});void drain();
}
async function drain() {
  if(sending)return;sending=true;
  try {
    while(outgoing.length){const item=outgoing.shift()!;if(!ready||item.epoch!==epoch)continue;
      try{await invoke('receiver_send',item);}catch{if(item.epoch===epoch)void fail('输入连接中断，正在重新连接。');break;}}
  } finally {sending=false;if(ready&&outgoing.length)void drain();}
}
async function fail(text:string,retry=true) {
  const token=++run;invalidate(text);
  await invoke('receiver_disconnect').catch(()=>{});
  if(retry&&!closing&&token===run){clearTimeout(retryTimer);retryTimer=setTimeout(()=>void connect(),2000);}
}
function refreshMembers() {
  members.replaceChildren(...[...catalog.values()].map(w=>{const option=document.createElement('option');option.value=String(w.window_id);option.textContent=w.title;return option;}));
  if(!catalog.has(selected))selected=[...catalog.values()].find(w=>w.parent_id===null)?.window_id??catalog.keys().next().value??0;
  members.value=String(selected);
  const current=catalog.get(selected);if(current)void local.setTitle(`${current.title} — 窗渡 PaneFerry`);
}
function release() {
  for(const key of pressed.values())transmit({...key,pressed:false});pressed.clear();
  for(const button of buttons)transmit({type:'input.pointer',...lastPoint,button,pressed:false});buttons.clear();
}
function focusRemote(){if(ready){transmit({type:'window.focus'});ime.focus({preventScroll:true});}}
members.onchange=()=>{release();selected=Number(members.value);displayed=undefined;waitingKey.add(selected);sentResize='';clearVideo();context.clearRect(0,0,canvas.width,canvas.height);empty.hidden=false;empty.textContent='等待画面…';refreshMembers();transmit({type:'video.keyframe'});focusRemote();};
function resize() {
  const window=catalog.get(selected);if(!ready||!window||size.value==='source')return;
  const requested=size.value==='auto'?resizePixels(view.clientWidth,view.clientHeight,window.scale_factor):{width:Number(size.value.split('x')[0]),height:Number(size.value.split('x')[1])};
  const key=`${selected}:${requested.width}:${requested.height}`;
  if(key===sentResize)return;sentResize=key;
  transmit({type:'window.configure',...requested});
}
let resizeTimer:ReturnType<typeof setTimeout>;
new ResizeObserver(()=>{clearTimeout(resizeTimer);resizeTimer=setTimeout(resize,180);}).observe(view);
size.onchange=()=>{sentResize='';resize();};
// Renegotiate on a fresh connection so both legacy and QoS publishers obey the
// chosen limit and start a new keyframe. The session itself remains unchanged.
fps.onchange=()=>void connect(Number(fps.value));
element<HTMLButtonElement>('retry').onclick=()=>void connect();
element<HTMLButtonElement>('close').onclick=async()=>{closing=true;clearTimeout(retryTimer);await fail('已断开连接。',false);await local.close();};
window.addEventListener('beforeunload',()=>{closing=true;ready=false;run++;clearVideo();void invoke('receiver_disconnect');});
window.addEventListener('blur',release);
document.addEventListener('visibilitychange',()=>{if(document.hidden)release();});
view.addEventListener('contextmenu',e=>e.preventDefault());
function location(event:PointerEvent|WheelEvent,clamp=false){const w=displayed;if(!w||w.window_id!==selected||w.geometry_revision!==catalog.get(selected)?.geometry_revision)return null;const rect=view.getBoundingClientRect();return pointer(event.clientX-rect.left,event.clientY-rect.top,rect.width,rect.height,w.width,w.height,clamp);}
view.onpointerdown=event=>{
  if(!ready||event.button>2)return;const point=location(event);if(!point)return;
  event.preventDefault();lastPoint=point;focusRemote();
  ime.style.left=`${Math.max(0,event.clientX-view.getBoundingClientRect().left)}px`;ime.style.top=`${Math.max(0,event.clientY-view.getBoundingClientRect().top)}px`;
  view.setPointerCapture(event.pointerId);const button=[1,2,3][event.button];buttons.add(button);transmit({type:'input.pointer',...point,button,pressed:true});
};
let motion:PointerEvent|undefined,motionFrame=0;
view.onpointermove=event=>{motion=event;if(!motionFrame)motionFrame=requestAnimationFrame(()=>{motionFrame=0;if(!motion)return;const point=location(motion,buttons.size>0);if(point){lastPoint=point;transmit({type:'input.pointer',...point,button:0,pressed:false});}});};
view.onpointerup=event=>{const button=[1,2,3][event.button];if(!buttons.delete(button))return;const point=location(event,true)??lastPoint;lastPoint=point;transmit({type:'input.pointer',...point,button,pressed:false});};
view.onlostpointercapture=()=>{for(const button of buttons)transmit({type:'input.pointer',...lastPoint,button,pressed:false});buttons.clear();};
view.addEventListener('wheel',event=>{
  event.preventDefault();const point=location(event);if(!point)return;
  const button=Math.abs(event.deltaY)>=Math.abs(event.deltaX)?(event.deltaY<0?4:5):(event.deltaX<0?6:7);
  transmit({type:'input.pointer',...point,button,pressed:true});transmit({type:'input.pointer',...point,button,pressed:false});
},{passive:false});
ime.addEventListener('compositionstart',()=>{composing=true;});
function commit(){if(composing||!ime.value)return;const text=ime.value;ime.value='';transmit({type:'input.text.commit',text});}
ime.addEventListener('compositionend',()=>{composing=false;setTimeout(commit,0);});
ime.oninput=commit;
ime.onpaste=event=>event.preventDefault();
ime.onkeydown=event=>{
  if(!ready||composing||event.isComposing||event.keyCode===229||event.key==='Process')return;
  if(Array.from(event.key).length===1&&!event.ctrlKey&&!event.altKey&&!event.metaKey)return;
  const symbol=keysym(event.key,event.code),name=logicalKey(event.key,event.code);
  const key=capabilities.includes('input.key.logical')?(name===null?null:{type:'input.key.logical',key:name}):(symbol===null?null:{type:'input.key',keysym:symbol});
  if(!key)return;
  event.preventDefault();pressed.set(event.code,key);transmit({...key,pressed:true});
};
ime.onkeyup=event=>{const key=pressed.get(event.code);if(!key)return;event.preventDefault();pressed.delete(event.code);transmit({...key,pressed:false});};

async function frame(event:Record<string,unknown>,token:number) {
  const wid=Number(event.window_id),w=catalog.get(wid);if(!w||!w.visible||wid!==selected)return;
  if(waitingKey.has(wid)&&event.key_frame!==true)return;
  let video=videos.get(wid);
  const changed=!video||video.stream!==event.stream_id||video.revision!==w.geometry_revision;
  const packet=inspectFrame(event,w,changed);if(!packet)return;waitingKey.delete(wid);
  if(changed){
    clearVideo(wid);const generation=++stamp;
    const config:VideoDecoderConfig={codec:packet.codec!,optimizeForLatency:true};
    if(!(await VideoDecoder.isConfigSupported(config)).supported)throw Error('decoder_unavailable');
    if(token!==run)return;
    const pending=new Map<number,number|undefined>();
    const decoder=new VideoDecoder({
      output:decoded=>{
        try{
          const feedback=pending.get(decoded.timestamp);pending.delete(decoded.timestamp);
          if(token!==run||videos.get(wid)?.stamp!==generation)return;
          if(decoded.displayWidth<w.width||decoded.displayHeight<w.height||decoded.displayWidth>w.width+1||decoded.displayHeight>w.height+1){void fail('视频尺寸无效。',false);return;}
          if(wid===selected){
            if(canvas.width!==w.width||canvas.height!==w.height){canvas.width=w.width;canvas.height=w.height;}
            context.drawImage(decoded,0,0,w.width,w.height,0,0,w.width,w.height);displayed=w;empty.hidden=true;
            painted++;const now=performance.now();if(now-rateStart>=1000){element<HTMLSpanElement>('rate').textContent=`${Math.round(painted*1000/(now-rateStart))} fps · ${w.width} × ${w.height}`;painted=0;rateStart=now;}
            if(feedback!==undefined&&capabilities.includes('video.qos'))transmit({type:'video.feedback',frame_id:feedback});
          }
        }finally{decoded.close();}
      },error:()=>{if(token===run)void fail('视频解码失败，正在重新连接。');}
    });
    decoder.configure(config);video={decoder,stream:Number(event.stream_id),revision:w.geometry_revision,pending,stamp:generation};videos.set(wid,video);
  }
  if(!video)return;
  // Backpressure preserves every dependency frame. Never discard a P frame and
  // then decode its dependants; only decoded surfaces may be superseded.
  while(video.decoder.decodeQueueSize>=4){await new Promise(r=>setTimeout(r,8));if(token!==run)return;}
  const timestamp=++clock;
  video.pending.set(timestamp,typeof event.frame_id==='number'?event.frame_id:undefined);
  if(video.pending.size>64)throw Error('decoder_backlog');
  video.decoder.decode(new EncodedVideoChunk({type:packet.key?'key':'delta',timestamp,data:packet.data}));
}
async function connect(chosenFps?:number) {
  clearTimeout(retryTimer);const token=++run;invalidate('正在连接远程应用…');
  try {
    if(typeof VideoDecoder==='undefined')throw Error('decoder_unavailable');
    const response=await invoke<{epoch:number;snapshot:{windows:Record<string,unknown>[];scale_factor?:number};capabilities:string[];max_fps:number;selected_fps:number}>('receiver_open',{fps:chosenFps});
    if(token!==run)return;
    epoch=response.epoch;capabilities=response.capabilities;fps.value=String(response.selected_fps);
    catalog=new Map(response.snapshot.windows.map(value=>{const w=metadata(value);return [w.window_id,w];}));
    refreshMembers();ready=true;sentResize='';message(`已连接 · 帧率上限 ${response.max_fps} · 复制粘贴使用远端剪贴板`);focusRemote();resize();
    while(token===run&&!closing){
      const event=await invoke<Record<string,unknown>>('receiver_next',{epoch});if(token!==run)return;
      if(event.type==='frame')await frame(event,token);
      else if(event.type==='window.created'||event.type==='window.metadata'){
        const w=metadata(event),old=catalog.get(w.window_id);
        if(old&&w.geometry_revision<old.geometry_revision)continue;
        if(!old||old.geometry_revision!==w.geometry_revision||!w.visible){clearVideo(w.window_id);if(selected===w.window_id){displayed=undefined;context.clearRect(0,0,canvas.width,canvas.height);}}
        catalog.set(w.window_id,w);refreshMembers();
      }else if(event.type==='window.destroyed'){
        clearVideo(Number(event.window_id));catalog.delete(Number(event.window_id));if(!catalog.size){await fail('远端窗口已关闭。',false);return;}const previous=selected;refreshMembers();if(previous!==selected){displayed=undefined;waitingKey.add(selected);transmit({type:'video.keyframe'});}
      }else if(event.type==='session.exited'){await fail('远端会话已结束。',false);return;}
      else if(event.type==='error'){
        const code=String(event.code);
        const terminal=['session_exited','session_expired','session_controlled','sharing_disabled','desktop_locked'];
        if(terminal.includes(code)){await fail('远端会话不可用，请检查共享状态后重新连接。',false);return;}
        message(code==='input_focus_required'?'请在远端前台打开所选窗口。':'远端未接受操作，请检查窗口状态。');
      }
    }
  }catch(error){if(token!==run)return;const reason=error instanceof Error?error.message:String(error);
    const terminal=['remote:session_exited','remote:session_expired','remote:session_controlled','remote:sharing_disabled','remote:desktop_locked'].includes(reason);
    await fail(reason==='decoder_unavailable'?'H.264 解码不可用，请安装或更新 Microsoft Edge WebView2 Runtime。':terminal?'远端会话不可用，请检查共享状态后重新连接。':'连接中断或画面无效，请检查远端设备。',!terminal&&reason!=='decoder_unavailable');}
}
void connect();
