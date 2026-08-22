// metal_grind.m — Metal host for the `search_b2b` compute shader (Apple Silicon).
// Drop-in equivalent of gpu_grind.c (OpenCL) — SAME stdin/stdout protocol so the Rust miner
// invokes it identically. Two modes:
//   oneshot : ./metal_grind <prevhash64> <ntime8_16> <work_root64> <bits> [device] [nonce_start] [span]
//   daemon  : ./metal_grind daemon <device>  → sets up Metal + compiles blake2b.metal ONCE, then reads
//             jobs from stdin ("<prevhash64> <ntime16> <work_root64> <bits> <nstart> <span>\n"), grinds
//             each, prints verified winning nonces (8-hex) to stdout followed by "END <ghs>\n".
// Every winner is re-verified with a reference BLAKE2b-256 in C (never trusts the GPU blindly).
// Build (macOS): clang -O2 -fobjc-arc metal_grind.m -o metal_grind -framework Metal -framework Foundation
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <sys/time.h>

static double now(void){ struct timeval t; gettimeofday(&t,0); return t.tv_sec+t.tv_usec*1e-6; }
static int hx(char c){ if(c>='0'&&c<='9')return c-'0'; if(c>='a'&&c<='f')return c-'a'+10; if(c>='A'&&c<='F')return c-'A'+10; return -1; }
static int hexbin(const char*s,uint8_t*out,int nbytes){ if((int)strlen(s)!=nbytes*2) return -1;
  for(int i=0;i<nbytes;i++){ int a=hx(s[2*i]),b=hx(s[2*i+1]); if(a<0||b<0)return -1; out[i]=(uint8_t)((a<<4)|b);} return 0; }

// ── reference BLAKE2b-256 (RFC 7693, one block, inlen<=128) — identical to gpu_grind.c ──
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
// target big-endian = (2^224-1) >> bits, plus the 4 big-endian words T[0]=MSW..T[3]=LSW
static void build_target(int bits, uint8_t tgt_be[32], uint64_t T[4]){
  uint8_t base[32]; memset(base,0,4); memset(base+4,0xff,28);   // 2^224-1 in BE
  int shb=bits/8, sbit=bits%8; memset(tgt_be,0,32);
  for(int i=0;i<32;i++){ int src=i-shb; uint16_t acc=0;
    if(src>=0) acc |= (uint16_t)base[src] >> sbit;
    if(src-1>=0 && sbit) acc |= ((uint16_t)base[src-1] << (8-sbit)) & 0xff;
    tgt_be[i]=(uint8_t)acc; }
  T[0]=T[1]=T[2]=T[3]=0;
  for(int j=0;j<8;j++){ T[0]=(T[0]<<8)|tgt_be[j]; T[1]=(T[1]<<8)|tgt_be[8+j]; T[2]=(T[2]<<8)|tgt_be[16+j]; T[3]=(T[3]<<8)|tgt_be[24+j]; }
}

// locate blake2b.metal next to the executable (argv0 dir) or CWD
static NSString* find_metal_src(const char* argv0){
  NSFileManager* fm=[NSFileManager defaultManager];
  NSMutableArray* cand=[NSMutableArray array];
  [cand addObject:@"blake2b.metal"];
  NSString* exe=[NSString stringWithUTF8String:argv0];
  [cand addObject:[[exe stringByDeletingLastPathComponent] stringByAppendingPathComponent:@"blake2b.metal"]];
  for(NSString* p in cand){ if([fm fileExistsAtPath:p]) return p; }
  return @"blake2b.metal";
}

typedef struct {
  id<MTLDevice> dev; id<MTLCommandQueue> q; id<MTLComputePipelineState> pso;
  id<MTLBuffer> mhdr; id<MTLBuffer> mout; char dn[128];
} Gpu;

static Gpu gpu_setup(int didx, const char* argv0){
  Gpu g; memset(&g,0,sizeof(g));
  NSArray<id<MTLDevice>>* devs = MTLCopyAllDevices();
  if(devs.count==0){ id<MTLDevice> d=MTLCreateSystemDefaultDevice(); if(d) devs=@[d]; }
  if((NSUInteger)didx>=devs.count){ fprintf(stderr,"metal device %d does not exist (have %lu)\n",didx,(unsigned long)devs.count); exit(2); }
  g.dev=devs[didx];
  snprintf(g.dn,sizeof(g.dn),"%s",[[g.dev name] UTF8String]);
  g.q=[g.dev newCommandQueue];
  NSError* err=nil;
  NSString* srcpath=find_metal_src(argv0);
  NSString* src=[NSString stringWithContentsOfFile:srcpath encoding:NSUTF8StringEncoding error:&err];
  if(!src){ fprintf(stderr,"cannot read %s: %s\n",[srcpath UTF8String],[[err localizedDescription] UTF8String]); exit(2); }
  id<MTLLibrary> lib=[g.dev newLibraryWithSource:src options:nil error:&err];
  if(!lib){ fprintf(stderr,"metal compile error:\n%s\n",[[err localizedDescription] UTF8String]); exit(2); }
  id<MTLFunction> fn=[lib newFunctionWithName:@"search_b2b"];
  if(!fn){ fprintf(stderr,"kernel search_b2b not found\n"); exit(2); }
  g.pso=[g.dev newComputePipelineStateWithFunction:fn error:&err];
  if(!g.pso){ fprintf(stderr,"pipeline error: %s\n",[[err localizedDescription] UTF8String]); exit(2); }
  g.mhdr=[g.dev newBufferWithLength:80 options:MTLResourceStorageModeShared];
  g.mout=[g.dev newBufferWithLength:256*sizeof(uint32_t) options:MTLResourceStorageModeShared];
  return g;
}

// grind [nstart, nstart+nspan) with the 80-byte header + target. Prints verified winners. Returns GH/s.
static double gpu_grind_range(Gpu*g, const uint8_t hdr[80], uint64_t T0,uint64_t T1,uint64_t T2,uint64_t T3,
                             const uint8_t tgt_be[32], uint64_t nstart, uint64_t nspan){
  memcpy([g->mhdr contents], hdr, 80);
  uint32_t iter=512; uint64_t per=((uint64_t)1<<22)*iter;
  uint64_t T[4]={T0,T1,T2,T3};
  uint64_t done=0, base=nstart; double t0=now();
  NSUInteger tg=g->pso.maxTotalThreadsPerThreadgroup; if(tg>256) tg=256; if(tg<1) tg=1;
  while(done<nspan){
    uint64_t span=(nspan-done<per)?(nspan-done):per;
    if(base>=(1ULL<<32)) break;
    if(base+span>(1ULL<<32)) span=(1ULL<<32)-base;
    uint64_t gs=(span+iter-1)/iter;
    ((uint32_t*)[g->mout contents])[0]=0;   // clear winner counter (Shared/unified memory)
    id<MTLCommandBuffer> cb=[g->q commandBuffer];
    id<MTLComputeCommandEncoder> enc=[cb computeCommandEncoder];
    [enc setComputePipelineState:g->pso];
    [enc setBuffer:g->mhdr offset:0 atIndex:0];
    [enc setBytes:&base length:sizeof(uint64_t) atIndex:1];
    [enc setBytes:&iter length:sizeof(uint32_t) atIndex:2];
    [enc setBytes:T length:sizeof(T) atIndex:3];
    [enc setBuffer:g->mout offset:0 atIndex:4];
    [enc dispatchThreads:MTLSizeMake(gs,1,1) threadsPerThreadgroup:MTLSizeMake(tg,1,1)];
    [enc endEncoding];
    [cb commit]; [cb waitUntilCompleted];
    uint32_t* out=(uint32_t*)[g->mout contents];
    uint32_t n=out[0]; if(n>255)n=255;
    for(uint32_t i=0;i<n;i++){
      uint64_t nonce=base+(uint64_t)out[1+i]; if(nonce>=(1ULL<<32))continue;
      uint8_t hh[80]; memcpy(hh,hdr,80);
      hh[32]=(uint8_t)nonce; hh[33]=(uint8_t)(nonce>>8); hh[34]=(uint8_t)(nonce>>16); hh[35]=(uint8_t)(nonce>>24);
      uint8_t dg[32]; blake2b256(dg,hh,80);
      if(be_le(dg,tgt_be)){ printf("%08llx\n",(unsigned long long)nonce); }
    }
    base+=span; done+=span;
  }
  double dt=now()-t0; return dt>0?(double)done/dt/1e9:0.0;
}

int main(int argc,char**argv){ @autoreleasepool {
  // ── daemon mode: persistent Metal, jobs over stdin ──
  if(argc>=3 && !strcmp(argv[1],"daemon")){
    Gpu g=gpu_setup(atoi(argv[2]), argv[0]);
    fprintf(stderr,"READY %s\n",g.dn); fflush(stderr);
    char line[512];
    while(fgets(line,sizeof(line),stdin)){
      char ph[80],nt[48],wr[80]; int bits; unsigned long long ns=0,sp=0;
      if(sscanf(line,"%78s %46s %78s %d %llu %llu",ph,nt,wr,&bits,&ns,&sp)!=6){ printf("END 0\n"); fflush(stdout); continue; }
      uint8_t prevhash[32],ntime8[8],work_root[32];
      if(hexbin(ph,prevhash,32)||hexbin(nt,ntime8,8)||hexbin(wr,work_root,32)||bits<0||bits>=224){ printf("END 0\n"); fflush(stdout); continue; }
      uint8_t hdr[80]; memcpy(hdr,prevhash,32); memset(hdr+32,0,8); memcpy(hdr+40,ntime8,8); memcpy(hdr+48,work_root,32);
      uint8_t tgt_be[32]; uint64_t T[4]; build_target(bits,tgt_be,T);
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
  uint64_t nstart = argc>6?strtoull(argv[6],0,10):0ULL;
  uint64_t nspan  = argc>7?strtoull(argv[7],0,10):(1ULL<<32);
  uint8_t hdr[80]; memcpy(hdr,prevhash,32); memset(hdr+32,0,8); memcpy(hdr+40,ntime8,8); memcpy(hdr+48,work_root,32);
  uint8_t tgt_be[32]; uint64_t T[4]; build_target(bits,tgt_be,T);
  Gpu g=gpu_setup(didx, argv[0]);
  double ghs=gpu_grind_range(&g,hdr,T[0],T[1],T[2],T[3],tgt_be,nstart,nspan);
  fprintf(stderr,"metal_grind · %s · bits=%d · swept %llu nonces (%.2f GH/s)\n",g.dn,bits,(unsigned long long)nspan,ghs);
  return 0;
} }
