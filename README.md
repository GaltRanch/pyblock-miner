# pyblockMiner

**A GPU miner for the PyBLØCK LOTTO BLAKE2b pool — Bitcoin BLAKE2b, solo lottery, non-custodial.**

A terminal (TUI) miner: you mine to **your own** Bitcoin address and keep **99.1%** of every block you find, straight in your address (PyBLØCK fee 0.9%). No accounts, no custody. It saturates all your NVIDIA GPUs and shows live hashrate, blocks found and difficulty.

```
⛏ Bitcoin BLAKE2b · solo lottery
PyBLØCK  LOTTO BLAKE2b   ● LIVE   pool.pyblock.xyz:23110
your address  bc1q…            you keep 99.1% · PyBLØCK fee 0.9%
┌ HASHRATE ┐ ┌ BLOCKS FOUND ┐ ┌ DIFFICULTY ┐
│  26.5 GH/s │ │      3       │ │   bits 2    │
GPU 0  RTX 4090   11.0 GH/s
GPU 1  RTX 5090   15.5 GH/s
```

> ⚠️ **Honest framing — read this.** BLAKE2b is a *proposed* proof-of-work change for Bitcoin (Bitcoin Knots [PR #359](https://github.com/bitcoinknots/bitcoin/pull/359)). It is **not merged and not active on mainnet — there is no activation date.** `pool.pyblock.xyz:23110` is a **regtest demo pool**: the coin it mines is **not real Bitcoin and has no value.** This exists so miners can test BLAKE2b mining and see that they get paid to their own address, ready for the day (if ever) the network changes its PoW. Don't trust, verify.

---

## Requirements

- An **NVIDIA GPU** (one or more) with the proprietary driver + OpenCL (`nvidia-smi` and `libOpenCL` available).
- **Rust** (`cargo`) — https://rustup.rs
- **gcc** (to build the OpenCL host)
- **Python 3** with `ecdsa` (only for the address generator): `pip install ecdsa`

## Build

```bash
./build.sh
```

This compiles the OpenCL grinder (`gpu/gpu_grind`) and the miner (`target/release/pyblockMiner`).

## 1) Get an address

Generate a Bitcoin address (this is your mining "username" — you keep the private key):

```bash
python3 tools/genaddr.py
```

It prints a `bc1q…` address and its WIF private key. Save the key — it's yours. (You can also use any Bitcoin address you already control.)

## 2) Mine

```bash
./target/release/pyblockMiner --addr <your_address>
```

Options:

| flag | default | meaning |
|------|---------|---------|
| `--addr <addr>` | *(required)* | your Bitcoin address — every block you find pays 99.1% here |
| `--pool <host:port>` | `pool.pyblock.xyz:23110` | the pool to mine on |
| `--gpus <N>` | auto (all detected) | how many GPUs to use |

Press **`q`** to quit. Run it in a real terminal (it's a full-screen TUI).

---

## How it works

- **Native SV1 stratum client** (Rust): connects to the pool, subscribes/authorizes with your address as the username, receives BLAKE2b work.
- **Work construction**: builds the Sia-style BLAKE2b work (`work_root = BLAKE2b(0x00 || coinb1 || extranonce)`) for each job.
- **N-GPU grinding**: spawns one persistent `gpu_grind` (OpenCL) process per GPU. The kernel is compiled **once**; the nonce space is split across GPUs proportionally to their speed, so every GPU stays busy. Each candidate is **verified on the CPU** with a reference BLAKE2b before being submitted (the miner never trusts the GPU blindly).
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
