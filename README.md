# pyblockMiner

**A GPU/CPU miner for the PyBLØCK LOTTO BLAKE2b pool — Bitcoin BLAKE2b, solo lottery, non-custodial.**

A terminal (TUI) miner: you mine to **your own mainnet** Bitcoin address and keep **99.1%** of every block you find, straight in your address (PyBLØCK pool fee 0.9%). No accounts, no custody. It saturates all your NVIDIA GPUs (and/or CPU cores) and shows live cards for your hashrate, blocks and difficulty — plus **live network cards**: how many miners are online and the network's total hashrate.

```
⛏ Bitcoin BLAKE2b · solo lottery
PyBLØCK  LOTTO BLAKE2b   ● LIVE   pool.pyblock.xyz:23110
your address  bc1q…      keep 99.1% · pool fee 0.9% · donation 2.0% hash → PyBLØCK
┌ YOUR HASHRATE ┐ ┌ BLOCKS FOUND ┐ ┌ DIFFICULTY ┐
│    26.5 GH/s    │ │      3       │ │   bits 2    │
┌ ◈ MINERS ONLINE ┐ ┌ ◈ NETWORK HASHRATE ┐ ┌ ◈ POOL HEIGHT ┐
│        7         │ │      184 GH/s        │ │    842 190    │
GPU 0  RTX 4090   11.0 GH/s
GPU 1  RTX 5090   15.5 GH/s
```

> ⚠️ **Honest framing — read this.** BLAKE2b is a *proposed* proof-of-work change for Bitcoin (Bitcoin Knots [PR #359](https://github.com/bitcoinknots/bitcoin/pull/359)). It is **not merged and not active on mainnet — there is no activation date.** Until it activates, `pool.pyblock.xyz:23110` is a **regtest demo pool**: the coin it mines is **not real Bitcoin and has no value.** This exists so miners can test BLAKE2b mining and see that they get paid to their own address, **ready for the day (if ever) the network changes its PoW.** Because it targets mainnet, the miner **requires a mainnet address** (`bc1…` / `1…` / `3…`) and refuses regtest/testnet addresses. Don't trust, verify.

---

## Requirements

- An **NVIDIA GPU** (one or more) with the proprietary driver + OpenCL (`nvidia-smi` and `libOpenCL`). **No GPU?** It falls back to CPU.
- **Rust** (`cargo`) — https://rustup.rs
- **gcc** (to build the OpenCL host)
- **OpenCL headers + ICD loader** (to compile the grinder). Many systems have the NVIDIA runtime but **not** the dev headers → the classic `CL/cl.h: No such file or directory` build error. Install them:
  - Debian/Ubuntu: `sudo apt install ocl-icd-opencl-dev opencl-headers`
  - Fedora: `sudo dnf install ocl-icd-devel opencl-headers`
  - Arch: `sudo pacman -S opencl-icd-loader opencl-headers`
- **Python 3** with `ecdsa` (only for the address generator): `pip install ecdsa`

## Build

```bash
./build.sh
```

This compiles the OpenCL grinder (`gpu/gpu_grind`) and the miner (`target/release/pyblockMiner`).

## 1) Get a mainnet address

Generate a Bitcoin **mainnet** address (this is your mining "username" — you keep the private key):

```bash
python3 tools/genaddr.py
```

It prints a `bc1…` address and its WIF private key. Save the key — it's yours. (You can also use any mainnet Bitcoin address you already control.)

## 2) Mine

```bash
./target/release/pyblockMiner --addr <your_mainnet_address>
```

Options:

| flag | default | meaning |
|------|---------|---------|
| `--addr <addr>` | *(required)* | your Bitcoin address for the chosen `--network` — every block you find pays 99.1% here |
| `--network <net>` | `mainnet` | which chain: `mainnet` (bc1…/1…/3…), `testnet4` (tb1…/m…/n…/2…), or `regtest` (bcrt1…/m…/n…/2…). Sets the accepted address type; the dev donation applies **only on mainnet** (testnet/regtest coins have no value). |
| `--pool <host:port>` | `pool.pyblock.xyz:23110` | the pool to mine on (selectable) |
| `--gpus <N>` | auto (all detected) | how many GPUs to use (`0` = none) |
| `--cpu` | off | also mine on the CPU (added as an extra worker) |
| `--cpu-threads <N>` | auto (all cores) | CPU threads to use |
| `--donate <pct>` | `2.0` | PyBLØCK hashrate donation percent (mainnet only, minimum 2.0, see below) |

The TUI shows a **network badge** (MAINNET / TESTNET4 / REGTEST), your address's live **balance** (from a public explorer), your hashrate/blocks, and live network cards (miners online + network hashrate).

Press **`q`** to quit. Run it in a real terminal (it's a full-screen TUI).

**No GPU?** It falls back to **CPU mining** automatically (much slower — CPUs do ~MH/s vs GPUs' GH/s, but it works). Force it with `--gpus 0`, or add CPU alongside your GPUs with `--cpu`.

---

## PyBLØCK hashrate donation (like xmrig)

pyblockMiner is free and open source. It supports PyBLØCK the same way [xmrig's `donate-level`](https://xmrig.com/docs/miner/config#donate-level) supports its developer — but **paid in hashrate, not satoshis**. By default it donates **2%** of your hashing to the PyBLØCK LOTTO BLAKE2b pool. The minimum is **2%**; raise it any time with `--donate <pct>`. 🙏

How it works (transparent, no hidden magic): the miner opens a **second** stratum session — always to the PyBLØCK pool (`pool.pyblock.xyz:23110`), **regardless of which pool you set as your primary with `--pool`** — and spends `donate%` of its sweeps there. So ~`donate%` of your hashrate mines for PyBLØCK; in solo lottery that means ~`donate%` of any blocks that fraction finds go to PyBLØCK. This is separate from any pool's own fee, and it's what keeps PyBLØCK running even if you point your primary at your own node/pool.

The pool and address are hardcoded in the source (`DONATE_POOL` / `DEV_DONATION_ADDR` in `src/main.rs`):

```
pool  pool.pyblock.xyz:23110
addr  1PyBLoCKdiaC46vD9CWcmxa3ey2VzSc5Q2
```

## How it works

- **Native SV1 stratum client** (Rust): connects to the pool, subscribes/authorizes with your address as the username, receives BLAKE2b work.
- **Work construction**: builds the BLAKE2b work (`work_root = BLAKE2b(0x00 || coinb1 || extranonce)`) for each job.
- **N-GPU + CPU grinding**: spawns one persistent `gpu_grind` (OpenCL) process per GPU. The kernel is compiled **once**; the nonce space is split across GPUs (and CPU) proportionally to their speed, so every device stays busy. Each candidate is **verified on the CPU** with a reference BLAKE2b before being submitted (the miner never trusts the GPU blindly).
- **Non-custodial payout**: the pool builds a coinbase that pays your address directly (99.1%) plus a 0.9% PyBLØCK fee output. The pool never holds your rewards.

## Files

```
src/main.rs        the miner (Rust + ratatui TUI)
gpu/gpu_grind.c    OpenCL host: search_b2b kernel driver, oneshot + persistent daemon modes
gpu/blake2b.cl     BLAKE2b-256 OpenCL kernel
tools/genaddr.py   Bitcoin address + WIF generator
build.sh           builds everything
```

## License

MIT — see [LICENSE](LICENSE).
