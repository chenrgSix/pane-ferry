/* Standalone validation helper, not a production encoder. Own test pixmaps only.
 * CPU variants make exactly one immutable snapshot copy from a persistent map.
 * GPU variant wraps a duplicated DMA-buf fd and never maps the input on CPU.
 */
#define _GNU_SOURCE
#include <gst/gst.h>
#include <gst/app/gstappsrc.h>
#include <gst/app/gstappsink.h>
#include <gst/video/gstvideometa.h>
#include <gst/allocators/gstdmabuf.h>
#include <linux/dma-buf.h>
#include <sys/mman.h>
#include <sys/ioctl.h>
#include <poll.h>
#include <fcntl.h>
#include <unistd.h>
#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    int fd;
    unsigned width, height, stride;
    size_t size;
    void *map;
} Input;
typedef struct {
    GstElement *pipeline, *source, *sink;
    unsigned width, height, fps, frame;
    int mode;
    int input_counter;
    char error[1024];
} Encoder;
static uint64_t now(void) {return gst_util_get_timestamp();}
static int sync_input(Input *in, int end) {
    struct dma_buf_sync request={.flags=DMA_BUF_SYNC_READ | (end?DMA_BUF_SYNC_END:DMA_BUF_SYNC_START)};
    int ret;
    do {ret=ioctl(in->fd,DMA_BUF_IOCTL_SYNC,&request);} while(ret<0 && (errno==EINTR || errno==EAGAIN));
    return ret;
}
Input *probe_input_new(int fd,unsigned w,unsigned h,unsigned stride,int cpu) {
    if(!w || !h || stride<w*4) return NULL;
    Input *in=calloc(1,sizeof(*in));
    if(!in) return NULL;
    in->fd=fcntl(fd,F_DUPFD_CLOEXEC,0);
    in->width=w;in->height=h;in->stride=stride;in->size=(size_t)stride*h;
    if(in->fd<0) {free(in);return NULL;}
    if(cpu) {
        in->map=mmap(NULL,in->size,PROT_READ,MAP_SHARED,in->fd,0);
        if(in->map==MAP_FAILED) {close(in->fd);free(in);return NULL;}
    }
    return in;
}
void probe_input_free(Input *in) {
    if(!in) return;
    if(in->map) munmap(in->map,in->size);
    close(in->fd);free(in);
}
int probe_readback(Input *in,void *target) {
    struct pollfd p={.fd=in->fd,.events=POLLIN};
    int ret;
    do {ret=poll(&p,1,2000);} while(ret<0 && errno==EINTR);
    if(ret<=0 || !(p.revents&POLLIN) || (p.revents&(POLLERR|POLLNVAL)) || !in->map || sync_input(in,0)) return -1;
    size_t row=(size_t)in->width*4;
    for(unsigned y=0;y<in->height;y++) memcpy((char*)target+y*row,(char*)in->map+y*in->stride,row);
    return sync_input(in,1);
}
Encoder *probe_encoder_new(unsigned w,unsigned h,unsigned fps,unsigned threads,int mode,const char *color) {
    gst_init(NULL,NULL);
    Encoder *e=calloc(1,sizeof(*e));
    if(!e) return NULL;
    e->width=w;e->height=h;e->fps=fps;e->mode=mode;
    uint64_t bitrate=(uint64_t)w*h*6*fps/60;
    bitrate=CLAMP(bitrate,4000000,48000000);
    char *codec;
    if(mode==0) codec=g_strdup_printf("videoconvert name=conversion n-threads=%u ! video/x-raw,format=I420 ! "
        "x264enc speed-preset=ultrafast tune=zerolatency threads=4 sliced-threads=true bframes=0 rc-lookahead=0 sync-lookahead=0 "
        "bitrate=%llu key-int-max=%u qp-max=28 byte-stream=true",threads,(unsigned long long)((bitrate+999)/1000),fps);
    else codec=g_strdup_printf("vapostproc name=conversion ! video/x-raw(memory:VAMemory),format=NV12%s%s ! "
        "vah264lpenc rate-control=cbr bitrate=%llu max-qp=28 key-int-max=%u b-frames=0 ref-frames=1 target-usage=4",
        color&&*color?",colorimetry=":"",color?color:"",(unsigned long long)((bitrate+999)/1000),fps);
    char *description=g_strdup_printf("appsrc name=source is-live=true format=time block=true max-buffers=1 "
        "caps=video/x-raw,format=BGRx,width=%u,height=%u,framerate=%u/1 ! %s ! "
        "h264parse config-interval=-1 ! video/x-h264,stream-format=byte-stream,alignment=au ! "
        "appsink name=sink sync=false max-buffers=1 drop=false enable-last-sample=false",w,h,fps,codec);
    GError *error=NULL;
    e->pipeline=gst_parse_launch(description,&error);
    g_free(codec);g_free(description);
    if(error) {g_strlcpy(e->error,error->message,sizeof(e->error));g_error_free(error);return e;}
    if(!e->pipeline) {g_strlcpy(e->error,"no pipeline",sizeof(e->error));return e;}
    e->source=gst_bin_get_by_name(GST_BIN(e->pipeline),"source");
    e->sink=gst_bin_get_by_name(GST_BIN(e->pipeline),"sink");
    if(gst_element_set_state(e->pipeline,GST_STATE_PLAYING)==GST_STATE_CHANGE_FAILURE)
        g_strlcpy(e->error,"pipeline state failed",sizeof(e->error));
    return e;
}
const char *probe_encoder_error(Encoder *e) {return e->error;}
int probe_input_counter(Encoder *e) {return e->mode<2?e->input_counter:-1;}
void probe_encoder_free(Encoder *e) {
    if(!e) return;
    if(e->pipeline) gst_element_set_state(e->pipeline,GST_STATE_NULL);
    if(e->source) gst_object_unref(e->source);
    if(e->sink) gst_object_unref(e->sink);
    if(e->pipeline) gst_object_unref(e->pipeline);
    free(e);
}
int probe_encode(Encoder *e,Input *in,void **bytes,size_t *length,int *key,uint64_t *points) {
    if(*e->error || in->width!=e->width || in->height!=e->height) return -1;
    points[0]=now();
    GstBuffer *buffer=NULL;
    if(e->mode>=2) {
        if(e->mode==3) {
            struct pollfd ready={.fd=in->fd,.events=POLLIN};
            int ret;
            do {ret=poll(&ready,1,2000);} while(ret<0 && errno==EINTR);
            if(ret<=0 || !(ready.revents&POLLIN) || (ready.revents&(POLLERR|POLLNVAL))) return -1;
        }
        GstAllocator *allocator=gst_dmabuf_allocator_new();
        int fd=fcntl(in->fd,F_DUPFD_CLOEXEC,0);
        if(fd<0) return -1;
        GstMemory *memory=gst_dmabuf_allocator_alloc(allocator,fd,in->size);
        gst_object_unref(allocator);
        if(!memory) {close(fd);return -1;}
        buffer=gst_buffer_new();gst_buffer_append_memory(buffer,memory);
        gsize offsets[GST_VIDEO_MAX_PLANES]={0};
        gint strides[GST_VIDEO_MAX_PLANES]={(gint)in->stride,0,0,0};
        if(!gst_buffer_add_video_meta_full(buffer,0,GST_VIDEO_FORMAT_BGRx,in->width,in->height,1,offsets,strides)) {
            gst_buffer_unref(buffer);return -1;
        }
    } else {
        buffer=gst_buffer_new_allocate(NULL,(size_t)in->width*in->height*4,NULL);
        GstMapInfo map=GST_MAP_INFO_INIT;
        if(!gst_buffer_map(buffer,&map,GST_MAP_WRITE)) {gst_buffer_unref(buffer);return -1;}
        int copied=probe_readback(in,map.data);
        if(!copied && in->width>=256 && in->height>=32) {
            e->input_counter=0;
            for(unsigned bit=0;bit<8;bit++)
                e->input_counter|=(map.data[((size_t)16*in->width+bit*32+16)*4+2]>128)<<bit;
        }
        gst_buffer_unmap(buffer,&map);
        if(copied) {gst_buffer_unref(buffer);return -1;}
    }
    points[1]=now();
    GST_BUFFER_PTS(buffer)=(uint64_t)e->frame*GST_SECOND/e->fps;
    GST_BUFFER_DURATION(buffer)=GST_SECOND/e->fps;
    e->frame++;
    if(gst_app_src_push_buffer(GST_APP_SRC(e->source),buffer)!=GST_FLOW_OK) return -1;
    GstSample *sample=gst_app_sink_try_pull_sample(GST_APP_SINK(e->sink),2*GST_SECOND);
    if(!sample) {
        GstBus *bus=gst_element_get_bus(e->pipeline);
        GstMessage *message=gst_bus_pop_filtered(bus,GST_MESSAGE_ERROR);
        if(message) {
            GError *error=NULL;char *debug=NULL;gst_message_parse_error(message,&error,&debug);
            g_strlcpy(e->error,error->message,sizeof(e->error));g_error_free(error);g_free(debug);gst_message_unref(message);
        } else g_strlcpy(e->error,"output timeout",sizeof(e->error));
        gst_object_unref(bus);return -1;
    }
    GstBuffer *packet=gst_sample_get_buffer(sample);
    *length=gst_buffer_get_size(packet);*bytes=malloc(*length);
    if(!*bytes) {gst_sample_unref(sample);return -1;}
    gst_buffer_extract(packet,0,*bytes,*length);
    *key=!GST_BUFFER_FLAG_IS_SET(packet,GST_BUFFER_FLAG_DELTA_UNIT);
    gst_sample_unref(sample);points[2]=now();
    return 0;
}
char *probe_conversion_caps(Encoder *e) {
    GstElement *conversion=gst_bin_get_by_name(GST_BIN(e->pipeline),"conversion");
    GstPad *pad=gst_element_get_static_pad(conversion,"src");
    GstCaps *caps=gst_pad_get_current_caps(pad);
    char *text=caps?gst_caps_to_string(caps):g_strdup("");
    if(caps) gst_caps_unref(caps);
    gst_object_unref(pad);gst_object_unref(conversion);
    return text;
}
void probe_free(void *p) {g_free(p);}
void probe_to_rgb(const unsigned char *source,unsigned char *target,size_t pixels) {
    for(size_t n=0;n<pixels;n++) {target[n*3]=source[n*4+2];target[n*3+1]=source[n*4+1];target[n*3+2]=source[n*4];}
}
double probe_mse(const unsigned char *a,const unsigned char *b,size_t bytes) {
    double sum=0;
    for(size_t n=0;n<bytes;n++) {int d=(int)a[n]-b[n];sum+=(double)d*d;}
    return sum/bytes;
}
