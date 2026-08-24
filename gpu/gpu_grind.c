// gpu_grind.c — OpenCL host for the `search_b2b` kernel. Two modes:
//   oneshot : ./gpu_grind <prevhash64> <ntime8_16> <work_root64> <bits> [device] [nonce_start] [span]
//   daemon  : ./gpu_grind daemon <device>   → sets up OpenCL + compiles the kernel ONCE, then reads jobs
//   list    : ./gpu_grind list   → prints every OpenCL GPU (NVIDIA/AMD/Intel), one per line (device detection)
//             from stdin ("<prevhash64> <ntime16> <work_root64> <bits> <nstart> <span>\n"), grinds each,
//             prints verified winning nonces (8-hex) to stdout followed by "END <ghs>\n". No per-job overhead.
// Every winner is verified with a reference BLAKE2b-256 in C (never trusts the GPU blindly).
#include <CL/cl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <sys/time.h>

static double now(){ struct timeval t; gettimeofday(&t,0); return t.tv_sec+t.tv_usec*1e-6; }
static void chk(cl_int e,const char*m){ if(e!=CL_SUCCESS){ fprintf(stderr,"ERR %d in %s\n",e,m); exit(2);} }
static char* slurp(const char*p,size_t*n){ FILE*f=fopen(p,"rb"); if(!f){perror(p);exit(2);}
  fseek(f,0,SEEK_END); long s=ftell(f); fseek(f,0,SEEK_SET); char*b=malloc(s+1); fread(b,1,s,f); b[s]=0; fclose(f); if(n)*n=s; return b; }
static int hx(char c){ if(c>='0'&&c<='9')return c-'0'; if(c>='a'&&c<='f')return c-'a'+10; if(c>='A'&&c<='F')return c-'A'+10; return -1; }
static int hexbin(const char*s,uint8_t*out,int nbytes){ if((int)strlen(s)!=nbytes*2) return -1;
  for(int i=0;i<nbytes;i++){ int a=hx(s[2*i]),b=hx(s[2*i+1]); if(a<0||b<0)return -1; out[i]=(uint8_t)((a<<4)|b);} return 0; }

// ── reference BLAKE2b-256 (RFC 7693, one block, inlen<=128) ──
static const uint64_t H_IV[8]={
  0x6a09e667f3bcc908ULL,0xbb67ae8584caa73bULL,0x3c6ef372fe94f82bULL,0xa54ff53a5f1d36f1ULL,
  0x510e527fade682d1ULL,0x9b05688c2b3e6c1fULL,0x1f83d9abfb41bd6bULL,0x5be0cd19137e2179ULL };
static const uint8_t H_SIG[12][16]={
 {0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15},{14,10,4,8,9,15,13,6,1,12,0,2,11,7,5,3},
 {11,8,12,0,5,2,15,13,10,14,3,6,7,1,9,4},{7,9,3,1,13,12,11,14,2,6,5,10,4,0,15,8},
 {9,0,5,7,2,4,10,15,14,1,11,12,6,8,3,13},{2,12,6,10,0,11,8,3,4,13,7,5,15,14,1,9},
 {12,5,1,15,14,13,4,10,0,7,6,3,9,2,8,11},{13,11,7,14,12,1,3,9,5,0,15,4,8,6,2,10},
 {6,15,14,9,11,3,0,8,12,2,13,7,1,4,10,5},{10,2,8,4,7,6,1,5,15,11,9,14,3,12,13,0},
 {0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15},{14,10,4,8,9,15,13,6,1,12,0,2,11,7,5,3} };
#define ROTR(x,n) (((x)>>(n))|((x)<<(64-(n))))
static void hG(uint64_t v[16],int a,int b,int c,int d,uint64_t x,uint64_t y){
  v[a]=v[a]+v[b]+x; v[d]=ROTR(v[d]^v[a],32); v[c]=v[c]+v[d]; v[b]=ROTR(v[b]^v[c],24);
  v[a]=v[a]+v[b]+y; v[d]=ROTR(v[d]^v[a],16); v[c]=v[c]+v[d]; v[b]=ROTR(v[b]^v[c],63);
}
static uint64_t ld64(const uint8_t*p){ uint64_t w=0; for(int j=0;j<8;j++) w|=((uint64_t)p[j])<<(8*j); return w; }
static void blake2b256(uint8_t out[32], const uint8_t* in, size_t inlen){
  uint64_t h[8]; for(int i=0;i<8;i++) h[i]=H_IV[i]; h[0]^=0x0000000001010020ULL;
  uint8_t block[128]; memset(block,0,128); memcpy(block,in,inlen);
  uint64_t v[16],m[16];
  for(int i=0;i<8;i++){ v[i]=h[i]; v[8+i]=H_IV[i]; }
  v[12]^=(uint64_t)inlen; v[14]^=~0ULL;
  for(int i=0;i<16;i++) m[i]=ld64(block+8*i);
  for(int r=0;r<12;r++){ const uint8_t*s=H_SIG[r];
    hG(v,0,4,8,12,m[s[0]],m[s[1]]);  hG(v,1,5,9,13,m[s[2]],m[s[3]]);
    hG(v,2,6,10,14,m[s[4]],m[s[5]]); hG(v,3,7,11,15,m[s[6]],m[s[7]]);
    hG(v,0,5,10,15,m[s[8]],m[s[9]]); hG(v,1,6,11,12,m[s[10]],m[s[11]]);
    hG(v,2,7,8,13,m[s[12]],m[s[13]]);hG(v,3,4,9,14,m[s[14]],m[s[15]]); }
  for(int i=0;i<8;i++) h[i]^=v[i]^v[8+i];
  for(int i=0;i<32;i++) out[i]=(uint8_t)(h[i/8]>>(8*(i%8)));
}
static int be_le(const uint8_t dg[32], const uint8_t tgt_be[32]){
  for(int i=0;i<32;i++){ if(dg[i]<tgt_be[i]) return 1; if(dg[i]>tgt_be[i]) return 0; } return 1;
}
// target big-endian = (2^224-1) >> bits, + the 4 big-endian words T[0]=MSW..T[3]=LSW
static void build_target(int bits, uint8_t tgt_be[32], cl_ulong T[4]){
  uint8_t base[32]; memset(base,0,4); memset(base+4,0xff,28);   // 2^224-1 in BE
  int shb=bits/8, sbit=bits%8;
  memset(tgt_be,0,32);
  for(int i=0;i<32;i++){
    int src=i-shb; uint16_t acc=0;
    if(src>=0) acc |= (uint16_t)base[src] >> sbit;
    if(src-1>=0 && sbit) acc |= ((uint16_t)base[src-1] << (8-sbit)) & 0xff;
    tgt_be[i]=(uint8_t)acc;
  }
  T[0]=T[1]=T[2]=T[3]=0;
  for(int j=0;j<8;j++){ T[0]=(T[0]<<8)|tgt_be[j]; T[1]=(T[1]<<8)|tgt_be[8+j]; T[2]=(T[2]<<8)|tgt_be[16+j]; T[3]=(T[3]<<8)|tgt_be[24+j]; }
}

typedef struct { cl_context ctx; cl_command_queue q; cl_kernel ks; cl_mem mhdr; cl_mem mout; char dn[128]; } Gpu;

// True if this platform is a discrete-GPU vendor (NVIDIA/AMD). We enumerate those FIRST so device 0 is your
// real card, not an Intel iGPU or a CPU-OpenCL platform. (Back-compat: NVIDIA-only boxes are unchanged.)
static int is_discrete_platform(cl_platform_id p){
  char nm[256]=""; clGetPlatformInfo(p,CL_PLATFORM_NAME,256,nm,0);
  char vd[256]=""; clGetPlatformInfo(p,CL_PLATFORM_VENDOR,256,vd,0);
  return strstr(nm,"NVIDIA")||strstr(nm,"CUDA")||strstr(nm,"AMD")||strstr(nm,"ATI")
      || strstr(vd,"NVIDIA")||strstr(vd,"Advanced Micro")||strstr(vd,"AMD");
}

// Enumerate ALL GPU devices across ALL OpenCL platforms into a GLOBAL list (any vendor: NVIDIA / AMD / Intel).
// Discrete vendors first, then the rest. Single source of truth for both `list` (detection) and `daemon <N>`
// (driving) → device index N means the same GPU in both.  Returns the device count.
static cl_uint enum_gpus(cl_device_id *out, char names[][128], cl_uint maxn){
  cl_platform_id plat[8]; cl_uint np=0;
  if(clGetPlatformIDs(8,plat,&np)!=CL_SUCCESS) return 0;
  cl_uint total=0;
  for(int pass=0; pass<2; pass++){
    for(cl_uint i=0;i<np && total<maxn;i++){
      if((pass==0) != (is_discrete_platform(plat[i])!=0)) continue;   // pass 0 = discrete; pass 1 = the rest
      cl_device_id devs[16]; cl_uint nd=0;
      if(clGetDeviceIDs(plat[i],CL_DEVICE_TYPE_GPU,16,devs,&nd)!=CL_SUCCESS) continue;
      for(cl_uint j=0;j<nd && total<maxn;j++){
        out[total]=devs[j]; names[total][0]=0;
        clGetDeviceInfo(devs[j],CL_DEVICE_NAME,128,names[total],0);
        total++;
      }
    }
  }
  return total;
}

static Gpu gpu_setup(int didx){
  Gpu g; memset(&g,0,sizeof(g)); cl_int e;
  cl_device_id devs[32]; char names[32][128];
  cl_uint nd = enum_gpus(devs, names, 32);
  if(nd==0){ fprintf(stderr,"no OpenCL GPU device found (install your vendor's OpenCL ICD: NVIDIA driver / AMD ROCm or AMDGPU / Intel)\n"); exit(2); }
  if((cl_uint)didx>=nd){ fprintf(stderr,"device %d does not exist (have %u)\n",didx,nd); exit(2); }
  cl_device_id dev=devs[didx];
  snprintf(g.dn,sizeof(g.dn),"%s",names[didx][0]?names[didx]:"OpenCL GPU");
  g.ctx=clCreateContext(0,1,&dev,0,0,&e); chk(e,"ctx");
  g.q=clCreateCommandQueue(g.ctx,dev,0,&e); chk(e,"queue");
  size_t src_n; char*src=slurp("blake2b.cl",&src_n);
  cl_program prog=clCreateProgramWithSource(g.ctx,1,(const char**)&src,&src_n,&e); chk(e,"prog");
  e=clBuildProgram(prog,1,&dev,"",0,0);
  if(e!=CL_SUCCESS){ char log[8192]; clGetProgramBuildInfo(prog,dev,CL_PROGRAM_BUILD_LOG,8192,log,0); fprintf(stderr,"BUILD:\n%s\n",log); exit(2); }
  g.ks=clCreateKernel(prog,"search_b2b",&e); chk(e,"ks");
  g.mhdr=clCreateBuffer(g.ctx,CL_MEM_READ_ONLY,80,0,&e); chk(e,"mhdr");
  g.mout=clCreateBuffer(g.ctx,CL_MEM_READ_WRITE,256*sizeof(cl_uint),0,&e); chk(e,"mout");
  return g;
}

// grind [nstart, nstart+nspan) with the given 80-byte header + target. Prints verified winners to stdout. Returns GH/s.
static double gpu_grind_range(Gpu*g, const uint8_t hdr[80], cl_ulong T0,cl_ulong T1,cl_ulong T2,cl_ulong T3, const uint8_t tgt_be[32], cl_ulong nstart, cl_ulong nspan){
  chk(clEnqueueWriteBuffer(g->q,g->mhdr,CL_TRUE,0,80,hdr,0,0,0),"whdr");
  cl_uint iter=512; size_t per=((size_t)1<<22)*iter;
  clSetKernelArg(g->ks,0,sizeof(g->mhdr),&g->mhdr);
  clSetKernelArg(g->ks,2,sizeof(cl_uint),&iter);
  clSetKernelArg(g->ks,3,sizeof(cl_ulong),&T0); clSetKernelArg(g->ks,4,sizeof(cl_ulong),&T1);
  clSetKernelArg(g->ks,5,sizeof(cl_ulong),&T2); clSetKernelArg(g->ks,6,sizeof(cl_ulong),&T3);
  clSetKernelArg(g->ks,7,sizeof(g->mout),&g->mout);
  cl_ulong done=0, base=nstart; double t0=now();
  while(done<nspan){
    cl_ulong span=(nspan-done<per)?(nspan-done):per;
    if(base>=(1ULL<<32)) break;
    if(base+span>(1ULL<<32)) span=(1ULL<<32)-base;
    size_t gs=(size_t)((span+iter-1)/iter);
    cl_uint zero=0; chk(clEnqueueWriteBuffer(g->q,g->mout,CL_TRUE,0,sizeof(cl_uint),&zero,0,0,0),"clr");
    clSetKernelArg(g->ks,1,sizeof(cl_ulong),&base);
    chk(clEnqueueNDRangeKernel(g->q,g->ks,1,0,&gs,0,0,0,0),"run");
    clFinish(g->q);
    cl_uint out[256]; chk(clEnqueueReadBuffer(g->q,g->mout,CL_TRUE,0,256*sizeof(cl_uint),out,0,0,0),"read");
    cl_uint n=out[0]; if(n>255)n=255;
    for(cl_uint i=0;i<n;i++){
      cl_ulong nonce=base+(cl_ulong)out[1+i]; if(nonce>=(1ULL<<32))continue;
      uint8_t hh[80]; memcpy(hh,hdr,80);
      hh[32]=(uint8_t)nonce; hh[33]=(uint8_t)(nonce>>8); hh[34]=(uint8_t)(nonce>>16); hh[35]=(uint8_t)(nonce>>24);
      uint8_t dg[32]; blake2b256(dg,hh,80);
      if(be_le(dg,tgt_be)) printf("%08llx\n",(unsigned long long)nonce);
    }
    base+=span; done+=span;
  }
  double dt=now()-t0; return dt>0?(double)done/dt/1e9:0.0;
}

int main(int argc,char**argv){
  // ── list mode: print every OpenCL GPU (any vendor), one per line — the miner uses this to detect devices ──
  if(argc>=2 && !strcmp(argv[1],"list")){
    cl_device_id devs[32]; char names[32][128];
    cl_uint nd=enum_gpus(devs,names,32);
    for(cl_uint i=0;i<nd;i++) printf("%s\n", names[i][0]?names[i]:"OpenCL GPU");
    return 0;
  }
  // ── daemon mode: persistent OpenCL, jobs over stdin ──
  if(argc>=3 && !strcmp(argv[1],"daemon")){
    Gpu g=gpu_setup(atoi(argv[2]));
    fprintf(stderr,"READY %s\n",g.dn); fflush(stderr);
    char line[512];
    while(fgets(line,sizeof(line),stdin)){
      char ph[80],nt[48],wr[80]; int bits; unsigned long long ns=0,sp=0;
      if(sscanf(line,"%78s %46s %78s %d %llu %llu",ph,nt,wr,&bits,&ns,&sp)!=6){ printf("END 0\n"); fflush(stdout); continue; }
      uint8_t prevhash[32],ntime8[8],work_root[32];
      if(hexbin(ph,prevhash,32)||hexbin(nt,ntime8,8)||hexbin(wr,work_root,32)||bits<0||bits>=224){ printf("END 0\n"); fflush(stdout); continue; }
      uint8_t hdr[80]; memcpy(hdr,prevhash,32); memset(hdr+32,0,8); memcpy(hdr+40,ntime8,8); memcpy(hdr+48,work_root,32);
      uint8_t tgt_be[32]; cl_ulong T[4]; build_target(bits,tgt_be,T);
      double ghs=gpu_grind_range(&g,hdr,T[0],T[1],T[2],T[3],tgt_be,ns,sp);
      printf("END %.2f\n",ghs); fflush(stdout);
    }
    return 0;
  }

  // ── oneshot mode (standalone / tests) ──
  if(argc<5){ fprintf(stderr,"usage: %s <prevhash64> <ntime8_16> <work_root64> <bits> [device] [nonce_start] [span]\n   or: %s daemon <device>\n",argv[0],argv[0]); return 2; }
  uint8_t prevhash[32],ntime8[8],work_root[32];
  if(hexbin(argv[1],prevhash,32)){ fprintf(stderr,"bad prevhash (64 hex)\n"); return 2; }
  if(hexbin(argv[2],ntime8,8)){ fprintf(stderr,"bad ntime8 (16 hex)\n"); return 2; }
  if(hexbin(argv[3],work_root,32)){ fprintf(stderr,"bad work_root (64 hex)\n"); return 2; }
  int bits=atoi(argv[4]); if(bits<0||bits>=224){ fprintf(stderr,"bits out of range\n"); return 2; }
  int didx = argc>5?atoi(argv[5]):0;
  cl_ulong nstart = argc>6?strtoull(argv[6],0,10):0ULL;
  cl_ulong nspan  = argc>7?strtoull(argv[7],0,10):(1ULL<<32);
  uint8_t hdr[80]; memcpy(hdr,prevhash,32); memset(hdr+32,0,8); memcpy(hdr+40,ntime8,8); memcpy(hdr+48,work_root,32);
  uint8_t tgt_be[32]; cl_ulong T[4]; build_target(bits,tgt_be,T);
  Gpu g=gpu_setup(didx);
  double ghs=gpu_grind_range(&g,hdr,T[0],T[1],T[2],T[3],tgt_be,nstart,nspan);
  fprintf(stderr,"gpu_grind · %s · bits=%d · swept %llu nonces (%.2f GH/s)\n",g.dn,bits,(unsigned long long)nspan,ghs);
  return 0;
}
