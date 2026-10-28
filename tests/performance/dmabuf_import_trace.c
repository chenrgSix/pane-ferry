/* Test process only. Builds on the VA timing observer; never reads pixels. */
#include "va_transfer_probe.c"
#include <va/va_drmcommon.h>
#include <gst/gst.h>
#include <gst/allocators/gstdmabuf.h>

VAStatus vaCreateSurfaces(VADisplay d, unsigned format, unsigned w, unsigned h,
                        VASurfaceID *surfaces, unsigned n,
                        VASurfaceAttrib *attributes, unsigned count) {
    typedef VAStatus (*Fn)(VADisplay,unsigned,unsigned,unsigned,VASurfaceID*,unsigned,VASurfaceAttrib*,unsigned);
    Fn fn=(Fn)real("vaCreateSurfaces");
    unsigned memory=0;
    VADRMPRIMESurfaceDescriptor *descriptor=NULL;
    for(unsigned i=0;i<count;i++) {
        if(attributes[i].type==VASurfaceAttribMemoryType)
            memory=attributes[i].value.value.i;
        if(attributes[i].type==VASurfaceAttribExternalBufferDescriptor)
            descriptor=attributes[i].value.value.p;
    }
    START;
    VAStatus status=fn(d,format,w,h,surfaces,n,attributes,count);
    int imported=(memory & VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2)!=0;
    FINISH(imported?"vaCreateSurfaces.prime2":"vaCreateSurfaces.other",w,memory);
    if(imported && descriptor && descriptor->num_layers && descriptor->layers[0].num_planes)
        emit("prime2.pitch",descriptor->width,descriptor->layers[0].pitch[0],
             begin,begin,0,status);
    return status;
}

static pthread_once_t gst_once=PTHREAD_ONCE_INIT;
static void *gst_library, *alloc_library;
static void gst_initialize(void) {
    gst_library=dlopen("libgstreamer-1.0.so.0",RTLD_NOW|RTLD_LOCAL);
    alloc_library=dlopen("libgstallocators-1.0.so.0",RTLD_NOW|RTLD_LOCAL);
    if(!gst_library || !alloc_library) abort();
}
gboolean gst_memory_map(GstMemory *memory,GstMapInfo *info,GstMapFlags flags) {
    pthread_once(&once,initialize);
    pthread_once(&gst_once,gst_initialize);
    typedef gboolean (*Fn)(GstMemory*,GstMapInfo*,GstMapFlags);
    typedef gboolean (*IsDma)(GstMemory*);
    Fn fn=(Fn)dlsym(gst_library,"gst_memory_map");
    IsDma isdma=(IsDma)dlsym(alloc_library,"gst_is_dmabuf_memory");
    if(!fn || !isdma) abort();
    int dma=isdma(memory);
    START;
    gboolean result=fn(memory,info,flags);
    VAStatus status=result?0:-1;
    if(dma) {FINISH("gst_memory_map.dmabuf",0,flags);}
    return result;
}
