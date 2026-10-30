// Coordinates are source pixels. The viewport uses logical CSS pixels, just as
// the AppKit host uses points; display DPI must not multiply remote input.
export type RemoteWindow = {window_id:number; width:number; height:number; geometry_revision:number; title:string; visible:boolean; parent_id:number|null;scale_factor:1|2};
export function dimensions(width:unknown, height:unknown): boolean {
  return Number.isSafeInteger(width) && Number.isSafeInteger(height) && Number(width)>0 && Number(height)>0 && Number(width)<=4096 && Number(height)<=2160 && Number(width)*Number(height)<=8_000_000;
}
export function metadata(value: Record<string, unknown>): RemoteWindow {
  if (!Number.isSafeInteger(value.window_id) || Number(value.window_id)<=0 || !dimensions(value.width,value.height) || !Number.isSafeInteger(value.geometry_revision) || Number(value.geometry_revision)<1) throw Error('invalid_metadata');
  if(value.scale_factor!==undefined&&value.scale_factor!==1&&value.scale_factor!==2)throw Error('invalid_scale');
  return {window_id:Number(value.window_id),width:Number(value.width),height:Number(value.height),geometry_revision:Number(value.geometry_revision),title:typeof value.title==='string'?value.title.slice(0,256):'远程应用',visible:value.visible!==false,parent_id:typeof value.parent_id==='number'?value.parent_id:null,scale_factor:value.scale_factor===2?2:1};
}
// Source pixels use source density; the local monitor's DPI is irrelevant.
export function sourceSize(window:RemoteWindow) {
  return {width:window.width/window.scale_factor,height:window.height/window.scale_factor};
}
export function imageRect(viewWidth:number, viewHeight:number, width:number, height:number) {
  const fit=Math.min(viewWidth/width,viewHeight/height);
  return {x:(viewWidth-width*fit)/2,y:(viewHeight-height*fit)/2,width:width*fit,height:height*fit};
}
export function pointer(x:number,y:number,vw:number,vh:number,width:number,height:number,clamp=false) {
  if (![x,y,vw,vh,width,height].every(Number.isFinite) || vw<=0 || vh<=0 || width<=0 || height<=0) return null;
  const rect=imageRect(vw,vh,width,height);
  if (!clamp && (x<rect.x || y<rect.y || x>=rect.x+rect.width || y>=rect.y+rect.height)) return null;
  return {x:Math.min(width-1,Math.max(0,Math.floor((x-rect.x)*width/rect.width))),y:Math.min(height-1,Math.max(0,Math.floor((y-rect.y)*height/rect.height)))};
}
export function resizePixels(width:number,height:number,scale=1) {
  width=Math.max(64,Math.round(width*scale)); height=Math.max(48,Math.round(height*scale));
  const fit=Math.min(1,4096/width,2160/height,Math.sqrt(8_000_000/width/height));
  return {width:Math.max(1,Math.floor(width*fit)),height:Math.max(1,Math.floor(height*fit))};
}
const keys:Record<string,number>={Backspace:0xff08,Tab:0xff09,Enter:0xff0d,Escape:0xff1b,Delete:0xffff,Insert:0xff63,Home:0xff50,End:0xff57,PageUp:0xff55,PageDown:0xff56,ArrowLeft:0xff51,ArrowUp:0xff52,ArrowRight:0xff53,ArrowDown:0xff54,Shift:0xffe1,Control:0xffe3,Alt:0xffe9,Meta:0xffeb,CapsLock:0xffe5};
export function logicalKey(key:string,code:string):string|null {
  const modifier:Record<string,string>={Shift:'Shift',Control:'Control',Alt:'Alt',Meta:'Super'};
  if(modifier[key])return `${modifier[key]}_${code.endsWith('Right')?'R':'L'}`;
  const names:Record<string,string>={Backspace:'BackSpace',Enter:'Return',PageUp:'Page_Up',PageDown:'Page_Down',ArrowLeft:'Left',ArrowRight:'Right',ArrowUp:'Up',ArrowDown:'Down',' ':'space',CapsLock:'Caps_Lock'};
  if(names[key])return names[key];
  if(['Tab','Escape','Delete','Insert','Home','End'].includes(key)||/^F([1-9]|1[0-9]|2[0-4])$/.test(key))return key;
  if(/^Key[A-Z]$/.test(code))return code.slice(3).toLowerCase();
  if(/^Digit[0-9]$/.test(code))return code.slice(5);
  const punctuation:Record<string,string>={Semicolon:'semicolon',Equal:'equal',Comma:'comma',Minus:'minus',Period:'period',Slash:'slash',Backquote:'grave',BracketLeft:'bracketleft',Backslash:'backslash',BracketRight:'bracketright',Quote:'apostrophe'};
  return punctuation[code]??null;
}
export function keysym(key:string,code:string):number|null {
  if (key in keys) return keys[key]+(code.endsWith('Right') && ['Shift','Control','Alt','Meta'].includes(key)?1:0);
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(key)) return 0xffbd+Number(key.slice(1));
  const chars=Array.from(key);
  if(chars.length!==1)return null;
  const point=chars[0].codePointAt(0)!;
  return point<=255?point:0x01000000|point;
}
export function annexB(data:Uint8Array):Uint8Array[] {
  const units:Uint8Array[]=[]; let start=-1;
  for(let i=0;i+2<data.length;i++) {
    const size=data[i]===0&&data[i+1]===0?(data[i+2]===1?3:data[i+2]===0&&data[i+3]===1?4:0):0;
    if(size) {if(start>=0&&i>start)units.push(data.subarray(start,i));start=i+size;i+=size-1;}
  }
  if(start>=0&&start<data.length)units.push(data.subarray(start));
  if(!units.length || units.length>256)throw Error('invalid_video');
  return units;
}
// Parse the SPS before handing untrusted media to the system decoder. This
// bounds coded dimensions and reference memory, including parameter updates.
export function spsInfo(nal:Uint8Array) {
  const bytes:number[]=[];
  for(let i=1;i<nal.length;i++){if(i>=3&&nal[i]===3&&nal[i-1]===0&&nal[i-2]===0)continue;bytes.push(nal[i]);}
  let bit=0;
  function bits(count:number){if(bit+count>bytes.length*8||count>32)throw Error('invalid_sps');let v=0;for(let i=0;i<count;i++,bit++)v=v*2+((bytes[bit>>3]>>(7-(bit&7)))&1);return v;}
  function ue(){let zeros=0;while(bits(1)===0){if(++zeros>24)throw Error('invalid_sps');}return 2**zeros-1+bits(zeros);}
  function se(){const v=ue();return v%2?(v+1)/2:-v/2;}
  const profile=bits(8),constraint=bits(8),level=bits(8);ue();
  let chroma=1;
  if([100,110,122,244,44,83,86,118,128,138,139,134,135].includes(profile)) {
    chroma=ue(); if(chroma!==1)throw Error('unsupported_chroma');
    if(ue()!==0||ue()!==0)throw Error('unsupported_depth');bits(1);
    if(bits(1))for(let i=0;i<8;i++)if(bits(1)){let last=8,next=8;for(let j=0;j<(i<6?16:64);j++){if(next!==0)next=(last+se()+256)%256;last=next===0?last:next;}}
  }
  ue();const order=ue();
  if(order===0)ue();else if(order===1){bits(1);se();se();const count=ue();if(count>256)throw Error('invalid_sps');for(let i=0;i<count;i++)se();}else if(order!==2)throw Error('invalid_sps');
  const references=ue();bits(1);const w=(ue()+1)*16,h=(ue()+1)*16;const progressive=bits(1);
  if(!progressive)throw Error('interlaced_video');bits(1);
  let left=0,right=0,top=0,bottom=0;if(bits(1)){left=ue();right=ue();top=ue();bottom=ue();}
  const width=w-(left+right)*2,height=h-(top+bottom)*2;
  if(!dimensions(width,height)||w>4096||h>2176||references>16||w*h*1.5*(references+1)>128*1024*1024)throw Error('invalid_sps');
  return {width,height,codec:`avc1.${[profile,constraint,level].map(v=>v.toString(16).padStart(2,'0')).join('')}`};
}
export function inspectFrame(event:Record<string,unknown>,window:RemoteWindow,needsKey:boolean) {
  if(event.geometry_revision!==window.geometry_revision)return null;
  if(event.codec!=='h264'||event.width!==window.width||event.height!==window.height||!Number.isSafeInteger(event.stream_id)||Number(event.stream_id)<=0||typeof event.key_frame!=='boolean'||typeof event.data!=='string'||event.data.length>16*1024*1024)throw Error('invalid_video');
  const ew=window.width+window.width%2,eh=window.height+window.height%2;
  if(event.encoded_width!==ew||event.encoded_height!==eh)throw Error('invalid_video');
  const data=Uint8Array.from(atob(event.data),c=>c.charCodeAt(0));
  if(!data.length||data.length>12*1024*1024)throw Error('invalid_video');
  const units=annexB(data),sps=units.filter(n=>(n[0]&31)===7);
  let codec:string|undefined;
  for(const nal of sps){const info=spsInfo(nal);if(info.width!==ew||info.height!==eh)throw Error('invalid_sps');codec=info.codec;}
  if(needsKey&&(!event.key_frame||!codec||!units.some(n=>(n[0]&31)===8)||!units.some(n=>(n[0]&31)===5)))throw Error('missing_keyframe');
  return {data,codec,key:event.key_frame};
}
