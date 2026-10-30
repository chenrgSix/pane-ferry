cbuffer Params : register(b0) {
    uint width, height, encodedWidth, encodedHeight;
    int canvasLeft, canvasTop, canvasWidth, canvasHeight;
    int surfaceLeft, surfaceTop, surfaceWidth, surfaceHeight;
    uint left, top, right, bottom;
    uint fullChroma, pad0, pad1, pad2;
};
Texture2D<float4> source : register(t0);
Texture2D<uint> composed : register(t1);
RWTexture2D<uint> canvas : register(u0);
RWByteAddressBuffer packed : register(u1);

[numthreads(16,16,1)]
void compose(uint3 id : SV_DispatchThreadID) {
    uint2 p=id.xy+uint2(left,top);
    if(p.x>=right || p.y>=bottom) return;
    int2 s=int2(canvasLeft,canvasTop)+int2(p)*int2(canvasWidth,canvasHeight)/int2(width,height)-int2(surfaceLeft,surfaceTop);
    s=clamp(s,int2(0,0),int2(surfaceWidth-1,surfaceHeight-1));
    uint4 rgba=(uint4)round(saturate(source.Load(int3(s,0)))*255.0);
    if(rgba.a==0) return;
    uint old=canvas[p];
    uint3 bg=uint3(old&255,(old>>8)&255,(old>>16)&255);
    uint3 c=min(uint3(255,255,255),rgba.bgr+(bg*(255-rgba.a)+127)/255);
    canvas[p]=c.x|(c.y<<8)|(c.z<<16)|0xff000000;
}
int3 rgb(uint x,uint y) {
    uint c=composed.Load(int3(min(x,width-1),min(y,height-1),0));
    return int3((c>>16)&255,(c>>8)&255,c&255);
}
uint yvalue(int3 c) { return ((66*c.r+129*c.g+25*c.b+128)>>8)+16; }
uint uvalue(int3 c) { return ((-38*c.r-74*c.g+112*c.b+128)>>8)+128; }
uint vvalue(int3 c) { return ((112*c.r-94*c.g-18*c.b+128)>>8)+128; }
uint value(uint offset) {
    uint area=encodedWidth*encodedHeight;
    if(fullChroma!=0) {
        uint w=width+width%2, n=w*(height+height%2);
        if(offset<n) return yvalue(rgb(offset%w,offset/w));
        if(offset<2*n) { uint i=offset-n; return uvalue(rgb(i%w,i/w)); }
        if(offset>=area && offset<area+n) { uint i=offset-area; return vvalue(rgb(i%w,i/w)); }
        return 128;
    }
    if(offset<area) return yvalue(rgb(offset%encodedWidth,offset/encodedWidth));
    uint i=offset-area, x=(i%encodedWidth)&~1, y=(i/encodedWidth)*2;
    int3 c=(rgb(x,y)+rgb(x+1,y)+rgb(x,y+1)+rgb(x+1,y+1))/4;
    return (i&1)==0 ? uvalue(c) : vvalue(c);
}
[numthreads(256,1,1)]
void convert(uint3 id : SV_DispatchThreadID) {
    uint offset=id.x*4, length=encodedWidth*encodedHeight*3/2;
    if(offset>=length) return;
    uint bits=0;
    [unroll] for(uint i=0;i<4;i++) if(offset+i<length) bits|=value(offset+i)<<(i*8);
    packed.Store(offset,bits);
}

// Write the encoder's NV12 texture directly. Both planes contain the same
// bytes as convert(), including the negotiated full-chroma carrier.
RWTexture2D<uint> luma : register(u2);
RWTexture2D<uint2> chroma : register(u3);
[numthreads(16,16,1)]
void convertTexture(uint3 id : SV_DispatchThreadID) {
    uint2 p=id.xy;
    if(p.x>=encodedWidth || p.y>=encodedHeight) return;
    luma[p]=value(p.y*encodedWidth+p.x);
    if(p.x<encodedWidth/2 && p.y<encodedHeight/2) {
        uint i=encodedWidth*encodedHeight+p.y*encodedWidth+2*p.x;
        chroma[p]=uint2(value(i),value(i+1));
    }
}
