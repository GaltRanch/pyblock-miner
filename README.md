# pyblockMiner

**A GPU/CPU miner for the PyBLØCK LOTTO BLAKE2b pool — Bitcoin BLAKE2b, solo lottery, non-custodial.**

A terminal (TUI) miner: you mine to **your own mainnet** Bitcoin address and keep **99.1%** of every block you find, straight in your address (PyBLØCK pool fee 0.9%). No accounts, no custody. It saturates all your NVIDIA GPUs (and/or CPU cores) and shows live hashrate, blocks found and difficulty.

```
⛏ Bitcoin BLAKE2b · solo lottery
PyBLØCK  LOTTO BLAKE2b   ● LIVE   pool.pyblock.xyz:23110
your address  bc1q…      keep 99.1% · pool fee 0.9% · dev donation 0.4%
┌ HASHRATE ┐ ┌ BLOCKS FOUND ┐ ┌ DIFFICULTY ┐
│  26.5 GH/s │ │      3       │ │   bits 2    │
GPU 0  RTX 4090   11.0 GH/s
GPU 1  RTX 5090   15.5 GH/s
```

> ⚠️ **Honest framing — read this.** BLAKE2b is a *proposed* proof-of-work change for Bitcoin (Bitcoin Knots [PR #359](https://github.com/bitcoinknots/bitcoin/pull/359)). It is **not merged and not active on mainnet — there is no activation date.** Until it activates, `pool.pyblock.xyz:23110` is a **regtest demo pool**: the coin it mines is **not real Bitcoin and has no value.** This exists so miners can test BLAKE2b mining and see that they get paid to their own address, **ready for the day (if ever) the network changes its PoW.** Because it targets mainnet, the miner **requires a mainnet address** (`bc1…` / `1…` / `3…`) and refuses regtest/testnet addresses. Don't trust, verify.

---

## Requirements

- An **NVIDIA GPU** (one or more) with the proprietary driver + OpenCL (`nvidia-smi` and `libOpenCL`). **No GPU?** It falls back to CPU.
- **Rust** (`cargo`) — https://rustup.rs
- **gcc** (to build the OpenCL host)
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
| `--addr <addr>` | *(required)* | your **mainnet** Bitcoin address — every block you find pays 99.1% here |
| `--pool <host:port>` | `pool.pyblock.xyz:23110` | the pool to mine on (selectable) |
| `--gpus <N>` | auto (all detected) | how many GPUs to use (`0` = none) |
| `--cpu` | off | also mine on the CPU (added as an extra worker) |
| `--cpu-threads <N>` | auto (all cores) | CPU threads to use |
| `--donate <pct>` | `0.4` | developer donation percent (minimum 0.4, see below) |

Press **`q`** to quit. Run it in a real terminal (it's a full-screen TUI).

**No GPU?** It falls back to **CPU mining** automatically (much slower — CPUs do ~MH/s vs GPUs' GH/s, but it works). Force it with `--gpus 0`, or add CPU alongside your GPUs with `--cpu`.

---

## Developer donation (like xmrig)

pyblockMiner is free and open source. By default it donates **0.4%** of your hashing to the developer — the same idea as [xmrig's `donate-level`](https://xmrig.com/docs/miner/config#donate-level). Raise it any time with `--donate <pct>`; the minimum is **0.4%**. 🙏

How it works (transparent, no hidden magic): the miner opens a **second** stratum session authorized to the developer's address and spends `donate%` of its sweeps there. In solo lottery that means **~`donate%` of any blocks you find pay the developer instead of you** — proportional, honest, and separate from the pool's 0.9% fee.

The donation address is hardcoded in the source (`DEV_DONATION_ADDR` in `src/main.rs`):

```
1PyBLoCKdiaC46vD9CWcmxa3ey2VzSc5Q2
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
