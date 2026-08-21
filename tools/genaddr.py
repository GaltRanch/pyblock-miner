#!/usr/bin/env python3
# pyblock_genaddr — generates a Bitcoin address + its private key (WIF), to use as your "username"
# when mining BLAKE2b on a PyBLØCK pool. You keep 99.1% of every block you find, straight to THAT
# address (PyBLØCK pool fee 0.9%). Non-custodial.
#   mainnet  → bc1q…   (default; --mainnet)
#   testnet4 → tb1q…   (--testnet4)  ← for mining the testnet4 BLAKE2b chain (coins have NO value)
# Usage: python3 pyblock_genaddr.py [--testnet4 | --network mainnet|testnet4] [count]
import sys, hashlib
from ecdsa import SigningKey, SECP256k1

def _sha256(b): return hashlib.sha256(b).digest()
def _hash160(b):
    h = hashlib.new('ripemd160'); h.update(_sha256(b)); return h.digest()

# ── base58check (WIF) ──
_B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
def b58check(payload):
    data = payload + _sha256(_sha256(payload))[:4]
    n = int.from_bytes(data, "big"); out = ""
    while n > 0: n, r = divmod(n, 58); out = _B58[r] + out
    return "1" * (len(data) - len(data.lstrip(b"\x00"))) + out

# ── bech32 (BIP173) ──
_CH = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"
def _polymod(v):
    GEN = [0x3b6a57b2,0x26508e6d,0x1ea119fa,0x3d4233dd,0x2a1462b3]; chk = 1
    for x in v:
        b = chk >> 25; chk = ((chk & 0x1ffffff) << 5) ^ x
        for i in range(5): chk ^= GEN[i] if ((b >> i) & 1) else 0
    return chk
def _hrp_expand(h): return [ord(c) >> 5 for c in h] + [0] + [ord(c) & 31 for c in h]
def _checksum(hrp, data):
    pm = _polymod(_hrp_expand(hrp) + data + [0]*6) ^ 1
    return [(pm >> 5*(5-i)) & 31 for i in range(6)]
def _convertbits(data, f, t):
    acc = bits = 0; ret = []; maxv = (1 << t) - 1
    for value in data:
        acc = (acc << f) | value; bits += f
        while bits >= t: bits -= t; ret.append((acc >> bits) & maxv)
    if bits: ret.append((acc << (t - bits)) & maxv)
    return ret
def encode_segwit(hrp, witver, prog):
    data = [witver] + _convertbits(list(prog), 8, 5)
    combined = data + _checksum(hrp, data)
    return hrp + "1" + "".join(_CH[d] for d in combined)

def gen(net):
    hrp     = "tb"      if net == "testnet4" else "bc"         # bech32 HRP
    wif_ver = b"\xef"   if net == "testnet4" else b"\x80"      # WIF network byte
    sk = SigningKey.generate(curve=SECP256k1)
    priv = sk.to_string()
    xy = sk.verifying_key.to_string(); x, y = xy[:32], xy[32:]
    pub = (b"\x02" if y[-1] % 2 == 0 else b"\x03") + x        # pubkey comprimida
    addr = encode_segwit(hrp, 0, _hash160(pub))               # p2wpkh
    wif = b58check(wif_ver + priv + b"\x01")                   # WIF comprimido
    return addr, wif

# ── args: [--testnet4 | --mainnet | --network <net>] [count] ──
net = "mainnet"; count = 1; a = sys.argv[1:]; i = 0
while i < len(a):
    if a[i] in ("--testnet4", "--t4", "--testnet"): net = "testnet4"
    elif a[i] == "--mainnet": net = "mainnet"
    elif a[i] == "--network" and i + 1 < len(a): i += 1; net = a[i]
    elif a[i].isdigit(): count = int(a[i])
    i += 1
if net not in ("mainnet", "testnet4"): net = "mainnet"
label = "TESTNET4" if net == "testnet4" else "MAINNET"

print("─" * 64)
print(f" PyBLØCK · BLAKE2b mining address · {label}")
print("─" * 64)
for k in range(max(1, count)):
    addr, wif = gen(net)
    if count > 1: print(f"\n #{k+1}")
    print(f"  Address (use this as your --addr in the miner):\n    {addr}")
    print(f"  Private key (WIF · save it, it's YOURS):\n    {wif}")
print("─" * 64)
print(" You mine to YOUR address → you keep 99.1% of every block · PyBLØCK pool fee 0.9%")
if net == "testnet4":
    print(" ⚠ TESTNET4: a real public BLAKE2b chain, but testnet coins have NO monetary value.")
    print("   Mine with:  pyblockMiner --network testnet4 --addr <this tb1…> --pool <host:port>")
else:
    print(" ⚠ MAINNET: until the BLAKE2b hardfork activates, mainnet is still SHA-256 (no BLAKE2b")
    print("   blocks yet, no date). Mine with:  pyblockMiner --addr <this bc1…>")
print("─" * 64)
