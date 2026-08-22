// BLAKE2b-256 (RFC 7693) OpenCL — kernel de validación + benchmark de minado.
// v3 (2026-08-12): rotaciones estilo sgminer-blake2b (zhq1) — cada ulong visto como uint2 (hi/lo) →
// las rotaciones usan shifts de 32-bit (nativos en NVIDIA) en vez del builtin rotate() de 64-bit.
// rot32 = swap de mitades (.yx) foldeado con el XOR (gratis). Todo OpenCL puro (sin CUDA).
// Estructura de bench/validación idéntica a v2 (la validación RFC 7693 corre primero = red de seguridad).

// rotate-right de 64 bits para y<32, sobre uint2 (x.x=low32, x.y=high32)
inline uint2 ror64(const uint2 x, const uint y){
  return (uint2)( (x.x>>y)^(x.y<<(32-y)), (x.y>>y)^(x.x<<(32-y)) );
}
// rotate-right de 64 bits para y>=32
inline uint2 ror64_2(const uint2 x, const uint y){
  return (uint2)( (x.y>>(y-32))^(x.x<<(64-y)), (x.x>>(y-32))^(x.y<<(64-y)) );
}

__constant ulong IV[8] = {
  0x6a09e667f3bcc908UL,0xbb67ae8584caa73bUL,0x3c6ef372fe94f82bUL,0xa54ff53a5f1d36f1UL,
  0x510e527fade682d1UL,0x9b05688c2b3e6c1fUL,0x1f83d9abfb41bd6bUL,0x5be0cd19137e2179UL };

__constant uchar SIGMA[12][16] = {
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

// G con rotaciones uint2. r,i literales (via ROUND desenrollada) → SIGMA[r][..] = índice de m constante.
// paso 1: d = rotr64(d^a,32) = swap de mitades (d.yx ^ a.yx). paso 2: b=ror64(b^c,24).
// paso 3: d=ror64(d^a,16). paso 4: b=ror64_2(b^c,63).
#define G(r,i,a,b,c,d) \
  a = a + b + m[SIGMA[r][2*i]]; \
  ((uint2*)&d)[0] = ((uint2*)&d)[0].yx ^ ((uint2*)&a)[0].yx; \
  c = c + d; \
  ((uint2*)&b)[0] = ror64( ((uint2*)&b)[0] ^ ((uint2*)&c)[0], 24U); \
  a = a + b + m[SIGMA[r][2*i+1]]; \
  ((uint2*)&d)[0] = ror64( ((uint2*)&d)[0] ^ ((uint2*)&a)[0], 16U); \
  c = c + d; \
  ((uint2*)&b)[0] = ror64_2( ((uint2*)&b)[0] ^ ((uint2*)&c)[0], 63U);

#define ROUND(r) \
  G(r,0,v[0],v[4],v[ 8],v[12]) \
  G(r,1,v[1],v[5],v[ 9],v[13]) \
  G(r,2,v[2],v[6],v[10],v[14]) \
  G(r,3,v[3],v[7],v[11],v[15]) \
  G(r,4,v[0],v[5],v[10],v[15]) \
  G(r,5,v[1],v[6],v[11],v[12]) \
  G(r,6,v[2],v[7],v[ 8],v[13]) \
  G(r,7,v[3],v[4],v[ 9],v[14])

inline void compress(ulong h[8], const ulong m[16], ulong t, int last){
  ulong v[16];
  for(int i=0;i<8;i++){ v[i]=h[i]; v[8+i]=IV[i]; }
  v[12]^=t;                       // t_hi=0 (mensajes <= 128B)
  if(last) v[14]^=0xFFFFFFFFFFFFFFFFUL;
  ROUND(0)  ROUND(1)  ROUND(2)  ROUND(3)
  ROUND(4)  ROUND(5)  ROUND(6)  ROUND(7)
  ROUND(8)  ROUND(9)  ROUND(10) ROUND(11)
  for(int i=0;i<8;i++) h[i]^= v[i]^v[8+i];
}

// byte-swap de 64 bits (LE<->BE)
inline ulong bswap64(ulong x){
  return ((x&0x00000000000000FFUL)<<56)|((x&0x000000000000FF00UL)<<40)|
         ((x&0x0000000000FF0000UL)<<24)|((x&0x00000000FF000000UL)<< 8)|
         ((x&0x000000FF00000000UL)>> 8)|((x&0x0000FF0000000000UL)>>24)|
         ((x&0x00FF000000000000UL)>>40)|((x&0xFF00000000000000UL)>>56);
}

// ── PyBLØCK LOTTO BLAKE2b miner: work[80]=prevhash[32]||nonce8[8]||ntime8[8]||work_root[32]. ────
// Differences vs `search`: (a) the nonce lives in work[32..39]=m[4] (nonce<2^32 → upper 32 bits are 0);
// (b) the pool verdict is int.from_bytes(BLAKE2b(work),"big") <= target  (compare_hashes with
//     byte[31]=MSB over share_hash=reverse(digest); target=(2^224-1)>>bits). We compare the digest
//     words in BIG-ENDIAN (b0=MSW) vs T0(MSW)..T3(LSW). Verified host-side + against the pool.
__kernel void search_b2b(__global const uchar* hdr, ulong nonce_base, uint iter,
                     ulong T0, ulong T1, ulong T2, ulong T3,   // target big-endian, T0=más significativa
                     volatile __global uint* out){
  uint gid=get_global_id(0);
  ulong m[16];
  for(int i=0;i<10;i++){ ulong w=0; for(int j=0;j<8;j++){ int idx=i*8+j; uchar b=(idx<80)?hdr[idx]:0; w|=((ulong)b)<<(8*j);} m[i]=w; }
  for(int i=10;i<16;i++) m[i]=0;
  ulong start = nonce_base + (ulong)gid*(ulong)iter;
  for(uint k=0;k<iter;k++){
    m[4]=start+k;                  // nonce en work[32..39]; el host garantiza start+k < 2^32
    ulong h[8];
    for(int i=0;i<8;i++) h[i]=IV[i];
    h[0]^=0x0000000001010020UL;
    compress(h,m,80,1);
    ulong b0=bswap64(h[0]),b1=bswap64(h[1]),b2=bswap64(h[2]),b3=bswap64(h[3]);   // digest big-endian (b0=MSW)
    bool win = (b0<T0) || (b0==T0 && (b1<T1 || (b1==T1 && (b2<T2 || (b2==T2 && b3<=T3)))));
    if(win){
      uint idx=atomic_inc(&out[0]);
      if(idx<255u) out[1u+idx]=gid*iter+k;
    }
  }
}
