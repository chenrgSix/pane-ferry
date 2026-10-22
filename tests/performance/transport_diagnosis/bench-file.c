#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <fcntl.h>
#include <string.h>
#include <errno.h>
int main(int argc,char**argv){
 if(argc<2)return 2;
 char path[4096];const char*dir=getenv("LRA_BENCH_DIR");if(!dir)dir="/tmp/lra-path-diagnosis";
 int upload=!strcmp(argv[1],"clipboard-upload");
 snprintf(path,sizeof(path),"%s/%s",dir,upload?"received.bin":"payload.bin");
 int fd=open(path,upload?(O_WRONLY|O_CREAT|O_TRUNC):O_RDONLY,0600);if(fd<0)return 3;
 char *b=malloc(65536);ssize_t n;int src=upload?0:fd,dst=upload?fd:1;
 while((n=read(src,b,65536))>0){ssize_t off=0;while(off<n){ssize_t k=write(dst,b+off,n-off);if(k<=0)return 4;off+=k;}}
 if(n<0)return 5;if(upload&&fsync(fd))return 6;close(fd);free(b);
 if(upload)puts("OK");return 0;
}
