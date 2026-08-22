// blake2b.metal — BLAKE2b-256 (RFC 7693) compute shader for Apple Silicon (Metal).
// Native Metal port of the `search_b2b` OpenCL kernel (gpu/blake2b.cl). Same contract:
//   work[80] = prevhash[32] || nonce8[8] (nonce in m[4], <2^32) || ntime8[8] || work_root[32]
//   winner  = int.from_bytes(BLAKE2b(work),"big") <= target   (digest big-endian vs T.x=MSW..T.w=LSW)
// Uses plain 64-bit rotates (Apple GPU has native 64-bit int; no NVIDIA uint2 trick needed).
#include <metal_stdlib>
using namespace metal;

constant ulong IV[8] = {
  0x6a09e667f3bcc908UL,0xbb67ae8584caa73bUL,0x3c6ef372fe94f82bUL,0xa54ff53a5f1d36f1UL,
  0x510e527fade682d1UL,0x9b05688c2b3e6c1fUL,0x1f83d9abfb41bd6bUL,0x5be0cd19137e2179UL };

constant uchar SIGMA[12][16] = {
 {0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15},
 {14,10,4,8,9,15,13,6,1,12,0,2,11,7,5,3},
 {11,8,12,0,5,2,15,13,10,14,3,6,7,1,9,4},
 {7,9,3,1,13,12,11,14,2,6,5,10,4,0,15,8},
 {9,0,5,7,2,4,10,15,14,1,11,12,6,8,3,13},
 {2,12,6,10,0,11,8,3,4,13,7,5,15,14,1,9},
 {12,5,1,15,14,13,4,10,0,7,6,3,9,2,8,11},
 {13,11,7,14,12,1,3,9,5,0,15,4,8,6,2,10},
 {6,15,14,9,11,3,0,8,12,2,13,7,1,4,10,5},
 {10,2,8,4,7,6,1,5,15,11,9,14,3,12,13,0},
 {0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15},
 {14,10,4,8,9,15,13,6,1,12,0,2,11,7,5,3} };

inline ulong rotr(ulong x, uint n){ return (x >> n) | (x << (64 - n)); }

inline void G(thread ulong* v, int a, int b, int c, int d, ulong x, ulong y){
  v[a] = v[a] + v[b] + x; v[d] = rotr(v[d]^v[a], 32); v[c] = v[c] + v[d]; v[b] = rotr(v[b]^v[c], 24);
  v[a] = v[a] + v[b] + y; v[d] = rotr(v[d]^v[a], 16); v[c] = v[c] + v[d]; v[b] = rotr(v[b]^v[c], 63);
}

// one-block compress (message <= 128 bytes, so t_hi = 0)
inline void compress(thread ulong* h, thread ulong* m, ulong t, bool last){
  ulong v[16];
  for(int i=0;i<8;i++){ v[i]=h[i]; v[8+i]=IV[i]; }
  v[12] ^= t;
  if(last) v[14] ^= 0xFFFFFFFFFFFFFFFFUL;
  for(int r=0;r<12;r++){
    G(v, 0,4, 8,12, m[SIGMA[r][ 0]], m[SIGMA[r][ 1]]);
    G(v, 1,5, 9,13, m[SIGMA[r][ 2]], m[SIGMA[r][ 3]]);
    G(v, 2,6,10,14, m[SIGMA[r][ 4]], m[SIGMA[r][ 5]]);
    G(v, 3,7,11,15, m[SIGMA[r][ 6]], m[SIGMA[r][ 7]]);
    G(v, 0,5,10,15, m[SIGMA[r][ 8]], m[SIGMA[r][ 9]]);
    G(v, 1,6,11,12, m[SIGMA[r][10]], m[SIGMA[r][11]]);
    G(v, 2,7, 8,13, m[SIGMA[r][12]], m[SIGMA[r][13]]);
    G(v, 3,4, 9,14, m[SIGMA[r][14]], m[SIGMA[r][15]]);
  }
  for(int i=0;i<8;i++) h[i] ^= v[i]^v[8+i];
}

inline ulong bswap64(ulong x){
  return ((x&0x00000000000000FFUL)<<56)|((x&0x000000000000FF00UL)<<40)|
         ((x&0x0000000000FF0000UL)<<24)|((x&0x00000000FF000000UL)<< 8)|
         ((x&0x000000FF00000000UL)>> 8)|((x&0x0000FF0000000000UL)>>24)|
         ((x&0x00FF000000000000UL)>>40)|((x&0xFF00000000000000UL)>>56);
}

// each thread sweeps `iter` consecutive nonces starting at nonce_base + gid*iter.
// hdr = 80-byte header. T = target big-endian (T.x = most-significant word .. T.w = least).
// out[0] = atomic winner counter; out[1..] = winning offsets (host: nonce = nonce_base + offset).
kernel void search_b2b(device const uchar*     hdr        [[buffer(0)]],
                       constant ulong&         nonce_base [[buffer(1)]],
                       constant uint&          iter       [[buffer(2)]],
                       constant ulong4&        T          [[buffer(3)]],
                       device atomic_uint*     out        [[buffer(4)]],
                       uint                    gid        [[thread_position_in_grid]])
{
  ulong m[16];
  for(int i=0;i<10;i++){ ulong w=0; for(int j=0;j<8;j++){ int idx=i*8+j; uchar b=(idx<80)?hdr[idx]:(uchar)0; w|=((ulong)b)<<(8*j);} m[i]=w; }
  for(int i=10;i<16;i++) m[i]=0;
  ulong start = nonce_base + (ulong)gid*(ulong)iter;
  for(uint k=0;k<iter;k++){
    m[4] = start + k;                       // nonce in work[32..39]; host guarantees start+k < 2^32
    ulong h[8];
    for(int i=0;i<8;i++) h[i]=IV[i];
    h[0] ^= 0x0000000001010020UL;           // param block: digest=32, key=0, fanout=1, depth=1
    compress(h, m, 80, true);
    ulong b0=bswap64(h[0]), b1=bswap64(h[1]), b2=bswap64(h[2]), b3=bswap64(h[3]);   // digest big-endian
    bool win = (b0<T.x) || (b0==T.x && (b1<T.y || (b1==T.y && (b2<T.z || (b2==T.z && b3<=T.w)))));
    if(win){
      uint idx = atomic_fetch_add_explicit(&out[0], 1u, memory_order_relaxed);
      if(idx < 255u) atomic_store_explicit(&out[1u+idx], gid*iter + k, memory_order_relaxed);
    }
  }
}
