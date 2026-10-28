/* Diagnostic LD_PRELOAD for a test-owned Agent only. No buffer data or addresses
 * are read. Compile against the installed libva version's public va.h.
 * VA calls and arguments pass through unchanged; trace timestamps add overhead.
 * PANEFERRY_VA_TRACE selects a fresh private JSONL file, never a production unit.
 */
#define _GNU_SOURCE
#include <va/va.h>
#include <dlfcn.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

static pthread_once_t once = PTHREAD_ONCE_INIT;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static void *library;
static int trace_fd = -1;
static uint64_t clock_ns(clockid_t clock) {
    struct timespec t;
    if (clock_gettime(clock, &t)) abort();
    return (uint64_t)t.tv_sec * 1000000000 + t.tv_nsec;
}
static void initialize(void) {
    library = dlopen("libva.so.2", RTLD_NOW | RTLD_LOCAL);
    if (!library) abort();
    const char *path = getenv("PANEFERRY_VA_TRACE");
    if (path) trace_fd = open(path, O_WRONLY | O_APPEND | O_CREAT | O_CLOEXEC, 0600);
    if (path && trace_fd < 0) abort();
}
static void *real(const char *name) {
    pthread_once(&once, initialize);
    void *fn = dlsym(library, name);
    if (!fn) abort();
    return fn;
}
static void emit(const char *name, uint32_t object, uint32_t aux,
                 uint64_t begin, uint64_t end, uint64_t cpu, VAStatus status) {
    if (trace_fd < 0) return;
    char row[384];
    int n = snprintf(row, sizeof(row),
        "{\"event\":\"%s\",\"pid\":%d,\"tid\":%ld,\"object\":%u,\"aux\":%u,"
        "\"begin_ns\":%llu,\"end_ns\":%llu,\"cpu_ns\":%llu,\"status\":%d}\n",
        name, getpid(), syscall(SYS_gettid), object, aux,
        (unsigned long long)begin, (unsigned long long)end, (unsigned long long)cpu, status);
    if (n < 0 || (size_t)n >= sizeof(row)) abort();
    pthread_mutex_lock(&lock);
    if (write(trace_fd, row, n) != n) abort();
    pthread_mutex_unlock(&lock);
}
typedef struct {
    int used;
    VAImageID image;
    VABufferID buffer;
    uint32_t bytes;
    uint64_t mapped_ns, mapped_cpu;
    long mapped_tid;
} Image;
static Image images[128];
static void remember(const VAImage *image) {
    pthread_mutex_lock(&lock);
    for (unsigned n = 0; n < 128; n++) if (!images[n].used) {
        images[n] = (Image){.used=1, .image=image->image_id, .buffer=image->buf, .bytes=image->data_size};
        pthread_mutex_unlock(&lock);
        return;
    }
    abort();
}
static uint32_t mapped(VABufferID buffer, uint64_t end, uint64_t cpu) {
    uint32_t bytes = 0;
    pthread_mutex_lock(&lock);
    for (unsigned n = 0; n < 128; n++) if (images[n].used && images[n].buffer == buffer) {
        images[n].mapped_ns = end;
        images[n].mapped_cpu = cpu;
        images[n].mapped_tid = syscall(SYS_gettid);
        bytes = images[n].bytes;
        break;
    }
    pthread_mutex_unlock(&lock);
    return bytes;
}
#define START uint64_t begin=clock_ns(CLOCK_MONOTONIC), cpu=clock_ns(CLOCK_THREAD_CPUTIME_ID)
#define FINISH(NAME, ID, AUX) do { \
    uint64_t end=clock_ns(CLOCK_MONOTONIC), used=clock_ns(CLOCK_THREAD_CPUTIME_ID)-cpu; \
    emit(NAME, ID, AUX, begin, end, used, status); \
} while(0)
#define WRAP(NAME, DECL, CALL, ID, AUX) \
VAStatus NAME DECL { \
    typedef VAStatus (*Fn) DECL; Fn fn=(Fn)real(#NAME); \
    START; VAStatus status=fn CALL; FINISH(#NAME, ID, AUX); return status; \
}
WRAP(vaBeginPicture, (VADisplay d, VAContextID c, VASurfaceID s), (d,c,s), c,s)
WRAP(vaRenderPicture, (VADisplay d, VAContextID c, VABufferID *b, int n), (d,c,b,n), c,n)
WRAP(vaEndPicture, (VADisplay d, VAContextID c), (d,c), c,0)
WRAP(vaSyncSurface, (VADisplay d, VASurfaceID s), (d,s), s,0)
WRAP(vaSyncSurface2, (VADisplay d, VASurfaceID s, uint64_t t), (d,s,t), s,0)
WRAP(vaSyncBuffer, (VADisplay d, VABufferID b, uint64_t t), (d,b,t), b,0)
WRAP(vaPutImage, (VADisplay d, VASurfaceID s, VAImageID i, int x, int y, unsigned w, unsigned h,
                 int dx, int dy, unsigned dw, unsigned dh), (d,s,i,x,y,w,h,dx,dy,dw,dh), s,i)
VAStatus vaCreateImage(VADisplay d, VAImageFormat *f, int w, int h, VAImage *i) {
    typedef VAStatus (*Fn)(VADisplay,VAImageFormat*,int,int,VAImage*);
    Fn fn=(Fn)real("vaCreateImage"); START;
    VAStatus status=fn(d,f,w,h,i);
    FINISH("vaCreateImage",status ? VA_INVALID_ID : i->buf,status ? 0 : i->data_size);
    if (!status) remember(i);
    return status;
}
VAStatus vaDeriveImage(VADisplay d, VASurfaceID s, VAImage *i) {
    typedef VAStatus (*Fn)(VADisplay,VASurfaceID,VAImage*);
    Fn fn=(Fn)real("vaDeriveImage"); START;
    VAStatus status=fn(d,s,i);
    FINISH("vaDeriveImage",s,status ? VA_INVALID_ID : i->buf);
    if (!status) remember(i);
    return status;
}
VAStatus vaDestroyImage(VADisplay d, VAImageID i) {
    typedef VAStatus (*Fn)(VADisplay,VAImageID);
    Fn fn=(Fn)real("vaDestroyImage"); START;
    VAStatus status=fn(d,i); FINISH("vaDestroyImage",i,0);
    if (!status) {
        pthread_mutex_lock(&lock);
        for (unsigned n=0;n<128;n++) if (images[n].used && images[n].image==i) images[n].used=0;
        pthread_mutex_unlock(&lock);
    }
    return status;
}
#define MAP(NAME, DECL, CALL) \
VAStatus NAME DECL { \
    typedef VAStatus (*Fn) DECL; Fn fn=(Fn)real(#NAME); START; \
    VAStatus status=fn CALL; \
    uint64_t end=clock_ns(CLOCK_MONOTONIC), used=clock_ns(CLOCK_THREAD_CPUTIME_ID); \
    uint32_t bytes=status ? 0 : mapped(b,end,used); \
    emit(#NAME,b,bytes,begin,end,used-cpu,status); return status; \
}
MAP(vaMapBuffer, (VADisplay d, VABufferID b, void **p), (d,b,p))
MAP(vaMapBuffer2, (VADisplay d, VABufferID b, void **p, uint32_t f), (d,b,p,f))
VAStatus vaUnmapBuffer(VADisplay d, VABufferID b) {
    typedef VAStatus (*Fn)(VADisplay,VABufferID);
    Fn fn=(Fn)real("vaUnmapBuffer"); START;
    Image image={0};
    pthread_mutex_lock(&lock);
    for (unsigned n=0;n<128;n++) if (images[n].used && images[n].buffer==b) {
        image=images[n]; images[n].mapped_ns=0; break;
    }
    pthread_mutex_unlock(&lock);
    if (image.mapped_ns && image.mapped_tid==syscall(SYS_gettid))
        emit("image.mapped_span",b,image.bytes,image.mapped_ns,begin,cpu-image.mapped_cpu,0);
    VAStatus status=fn(d,b); FINISH("vaUnmapBuffer",b,0); return status;
}
