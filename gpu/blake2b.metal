// blake2b.metal — BLAKE2b-256 (RFC 7693) compute shader for Apple Silicon (Metal).
// Native Metal port of the `search_b2b` OpenCL kernel (gpu/blake2b.cl). Same contract:
//   work[80] = prevhash[32] || nonce8[8] (nonce in m[4], <2^32) || ntime8[8] || work_root[32]
//   winner  = int.from_bytes(BLAKE2b(work),"big") <= target   (digest big-endian vs T.x=MSW..T.w=LSW)
// v2 (2026-08-24): +90% en Apple Silicon (M5 Max: 0.98 → 1.86 GH/s) —
//   · 12 rondas desenrolladas via macros (como el .cl): SIGMA[r][i] con r,i literales se constant-foldea
//     → m[] y v[] viven en registros. El bucle runtime `for r` forzaba indexado dinámico de m[] (stack).
//   · m[10..15]==0 siempre (mensaje de 80B) → MW() elide esos adds en compile-time.
//   · ronda 0: las columnas G0/G1/G3 no tocan m[4] (único word que cambia por nonce) y el estado
//     inicial es fijo → precomputadas una vez por thread, fuera del bucle de nonces.
//   · ronda 11: el último b=rotr(b^c,63) de las 4 G diagonales solo alimenta v[4..7], que la salida
//     (h[0..3]) no usa → omitido (GF). La salida solo calcula h[0..3] (lo único que se compara).
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

// mensaje de 80 bytes → m[10..15] son SIEMPRE 0; con índice literal el compilador elide el add.
#define MW(idx) ((idx) < 10 ? m[(idx) < 10 ? (idx) : 0] : 0UL)

#define G(r,i,a,b,c,d) \
  a = a + b + MW(SIGMA[r][2*i]);   d = rotr(d^a,32); c = c + d; b = rotr(b^c,24); \
  a = a + b + MW(SIGMA[r][2*i+1]); d = rotr(d^a,16); c = c + d; b = rotr(b^c,63);

// G final: omite el último b=rotr(b^c,63) — en la ronda 11 diagonal, b (v[4..7]) no llega a la salida
#define GF(r,i,a,b,c,d) \
  a = a + b + MW(SIGMA[r][2*i]);   d = rotr(d^a,32); c = c + d; b = rotr(b^c,24); \
  a = a + b + MW(SIGMA[r][2*i+1]); d = rotr(d^a,16); c = c + d;

#define ROUND(r) \
  G(r,0,v[0],v[4],v[ 8],v[12]) \
  G(r,1,v[1],v[5],v[ 9],v[13]) \
  G(r,2,v[2],v[6],v[10],v[14]) \
  G(r,3,v[3],v[7],v[11],v[15]) \
  G(r,4,v[0],v[5],v[10],v[15]) \
  G(r,5,v[1],v[6],v[11],v[12]) \
  G(r,6,v[2],v[7],v[ 8],v[13]) \
  G(r,7,v[3],v[4],v[ 9],v[14])

#define ROUND_LAST(r) \
  G(r,0,v[0],v[4],v[ 8],v[12]) \
  G(r,1,v[1],v[5],v[ 9],v[13]) \
  G(r,2,v[2],v[6],v[10],v[14]) \
  G(r,3,v[3],v[7],v[11],v[15]) \
  GF(r,4,v[0],v[5],v[10],v[15]) \
  GF(r,5,v[1],v[6],v[11],v[12]) \
  GF(r,6,v[2],v[7],v[ 8],v[13]) \
  GF(r,7,v[3],v[4],v[ 9],v[14])

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
  const ulong h0 = IV[0] ^ 0x0000000001010020UL;   // param block: digest=32, key=0, fanout=1, depth=1
  ulong start = nonce_base + (ulong)gid*(ulong)iter;
  // ronda 0, columnas 0/1/3: no tocan m[4] (nonce) y el estado inicial es fijo → precomputa 1 vez por thread
  ulong pv[16];
  pv[0]=h0; for(int i=1;i<8;i++) pv[i]=IV[i];
  for(int i=0;i<8;i++) pv[8+i]=IV[i];
  pv[12] ^= 80UL;                       // t = 80 bytes (un solo bloque)
  pv[14] ^= 0xFFFFFFFFFFFFFFFFUL;       // last block
  G(0,0,pv[0],pv[4],pv[ 8],pv[12])
  G(0,1,pv[1],pv[5],pv[ 9],pv[13])
  G(0,3,pv[3],pv[7],pv[11],pv[15])
  for(uint k=0;k<iter;k++){
    m[4] = start + k;                   // nonce en work[32..39]; host garantiza start+k < 2^32
    ulong v[16];
    for(int i=0;i<16;i++) v[i]=pv[i];
    G(0,2,v[2],v[6],v[10],v[14])
    G(0,4,v[0],v[5],v[10],v[15])
    G(0,5,v[1],v[6],v[11],v[12])
    G(0,6,v[2],v[7],v[ 8],v[13])
    G(0,7,v[3],v[4],v[ 9],v[14])
    ROUND(1)  ROUND(2)  ROUND(3)
    ROUND(4)  ROUND(5)  ROUND(6)  ROUND(7)
    ROUND(8)  ROUND(9)  ROUND(10) ROUND_LAST(11)
    ulong b0=bswap64(h0    ^ v[0]^v[ 8]);
    ulong b1=bswap64(IV[1] ^ v[1]^v[ 9]);
    ulong b2=bswap64(IV[2] ^ v[2]^v[10]);
    ulong b3=bswap64(IV[3] ^ v[3]^v[11]);
    bool win = (b0<T.x) || (b0==T.x && (b1<T.y || (b1==T.y && (b2<T.z || (b2==T.z && b3<=T.w)))));
    if(win){
      uint idx = atomic_fetch_add_explicit(&out[0], 1u, memory_order_relaxed);
      if(idx < 255u) atomic_store_explicit(&out[1u+idx], gid*iter + k, memory_order_relaxed);
    }
  }
}
