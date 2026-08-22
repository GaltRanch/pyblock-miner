# pyblockMiner

**A GPU/CPU miner for the PyBLØCK LOTTO BLAKE2b pool — Bitcoin BLAKE2b, solo lottery, non-custodial.**

A terminal (TUI) miner: you mine to **your own** Bitcoin address and keep **99.1%** of every block you find, straight in your address (PyBLØCK pool fee 0.9%). No accounts, no custody. It saturates all your NVIDIA GPUs (and/or CPU cores) and shows live cards for your hashrate, blocks and difficulty — plus **live network cards**: how many miners are online and the network's total hashrate.

It's a **tabbed app** — everything is in the program, no restarts needed:

```
PyBLØCK  1·MINE  2·STRATUMS  3·LEARN  4·NETWORK  5·SETUP  6·HELP
┌ ⛏ Bitcoin BLAKE2b · solo lottery ──────────────────────────────────┐
│ LOTTO BLAKE2b   TESTNET4   ● LIVE   pool.pyblock.xyz:23111          │
│ your address  tb1q…   balance 0.00000000 BTC   keep 99.1% · fee 0.9%│
├ YOUR HASHRATE ┤ ├ BLOCKS FOUND ┤ ├ DIFFICULTY ┤                     │
│    26.5 GH/s   │ │      3       │ │   bits 2   │                     │
├ ◈ MINERS ONLINE ┤ ├ ◈ NETWORK HASHRATE ┤ ├ ◈ POOL HEIGHT ┤         │
│        7         │ │      184 GH/s        │ │    149 460    │        │
│ GPU 0  RTX 4090   11.0 GH/s    GPU 1  RTX 5090   15.5 GH/s          │
└────────────────────────────────────────────────────────────────────┘
```

- **STRATUMS** — pick a pool and **switch it live, without leaving the miner.** PyBLØCK's pools are there by default (mainnet / testnet4 / regtest); add your own custom stratums too.
- **SETUP** — generate or paste your address, per network; toggle CPU and the donation. Everything **auto-saves** to a config file, so you don't re-type flags.
- **LEARN / HELP** — what BLAKE2b is, the honest hardfork status, and troubleshooting (incl. the OpenCL-headers fix).

> ⚠️ **Honest framing — read this.** BLAKE2b is a *proposed* proof-of-work change for Bitcoin (Bitcoin Knots [PR #359](https://github.com/bitcoinknots/bitcoin/pull/359)). It is **not merged and not active on Bitcoin mainnet — there is no activation date.** Mainnet is still SHA-256. The **first public chain** where you can actually mine BLAKE2b is **testnet4**, where the change activates at **block 149460** (Bitcoin Knots 29.4.1 RC) — but **testnet4 coins have no monetary value.** `pool.pyblock.xyz:23110` is a **regtest demo** (coin is not real Bitcoin, no value). This exists so miners can test BLAKE2b mining and see they get paid to their own address, **ready for the day (if ever) mainnet changes its PoW.** Each network needs its own address type (`bc1…` mainnet · `tb1…` testnet4 · `bcrt1…` regtest); the dev donation applies **only on mainnet**. Don't trust, verify.

---

## Requirements

- **Rust** (`cargo`) — https://rustup.rs
- **GPU (optional — no GPU falls back to CPU):**
  - **Linux:** an OpenCL GPU (NVIDIA / AMD / Intel). Needs `gcc` + OpenCL headers + ICD loader to build the grinder:
    - Debian/Ubuntu: `sudo apt install ocl-icd-opencl-dev opencl-headers`
    - Fedora: `sudo dnf install ocl-icd-devel opencl-headers`
    - Arch: `sudo pacman -S opencl-icd-loader opencl-headers`
    - Missing headers/lib? `./build.sh` just builds CPU-only (no abort).
  - **macOS (Apple Silicon):** the Metal grinder builds with the Xcode command-line tools (`xcode-select --install`). No OpenCL needed.
  - **Windows:** an OpenCL GPU. Build the grinder (`gpu\gpu_grind.exe`) with either MinGW-w64 `gcc` (able to link `-lOpenCL`) **or** MSVC Build Tools + the NVIDIA CUDA Toolkit (defines `CUDA_PATH`, ships `OpenCL.lib`). No toolchain? `build.bat` just builds CPU-only.
- Address generation is **native (Rust)** — no Python required.

## Build

```bash
./build.sh           # Linux / macOS
build.bat            # Windows
```

On Linux this builds the OpenCL grinder (`gpu/gpu_grind`); on macOS the Metal grinder (`gpu/metal_grind`); on Windows the OpenCL grinder (`gpu\gpu_grind.exe`); plus the miner (`target/release/pyblockMiner`, or `pyblockMiner.exe` on Windows). It falls back to CPU-only if the GPU toolchain isn't available. The miner auto-appends the `.exe` suffix when locating the grinder on Windows.

## 1) Get an address

Easiest: launch the miner, go to **SETUP** (`5`), press **`g`** to generate an address for the selected network (or **`e`** to paste one you already control). It's saved for you.

Or from the shell (native — this is your mining "username"; you keep the private key):

```bash
./target/release/pyblockMiner --genaddr mainnet    # bc1…
./target/release/pyblockMiner --genaddr testnet4   # tb1…
./target/release/pyblockMiner --genaddr regtest    # bcrt1…
```

It prints the address and its WIF private key (also saved to `~/.config/pyblockminer/keys.txt`, mode `0600`). Save the key — it's yours.

## 2) Mine

```bash
./target/release/pyblockMiner
```

That's it — on first run it opens with PyBLØCK's default stratums. Use **SETUP** to set your address and **STRATUMS** to pick/switch pools; your choices **persist** in `~/.config/pyblockminer/config.json`, so next time you just run `pyblockMiner`.

Flags are **optional overrides** (the saved config is otherwise the source of truth):

| flag | default | meaning |
|------|---------|---------|
| `--addr <addr>` | *(from config / SETUP)* | your Bitcoin address for the selected network — every block you find pays 99.1% here |
| `--network <net>` | *(from selected stratum)* | selects the stratum for that chain: `mainnet` (bc1…/1…/3…), `testnet4` (tb1…/m…/n…/2…), or `regtest` (bcrt1…/m…/n…/2…). The dev donation applies **only on mainnet** (testnet/regtest coins have no value). |
| `--pool <host:port>` | *(selected stratum's URL)* | override the selected stratum's URL |
| `--gpus <N>` | auto (all detected) | how many GPUs to use (`0` = CPU only) |
| `--cpu` | off | also mine on the CPU (added as an extra worker) |
| `--donate <pct>` | `2.0` | PyBLØCK hashrate donation percent (mainnet only, minimum 2.0, see below) |

### Keys

| key | action |
|-----|--------|
| `1`–`6` / `Tab` | switch tabs (MINE · STRATUMS · LEARN · NETWORK · SETUP · HELP) |
| `q` / `Esc` | quit (`Esc` also cancels a text input) |
| STRATUMS: `↑↓` `Enter` `a` `d` | move · **switch live** · add custom · delete custom |
| SETUP: `g` `e` `c` `+/-` | generate address · edit/paste address · toggle CPU · donation |
| LEARN: `←` `→` | previous / next info page |

The **MINE** tab shows a network badge (MAINNET / TESTNET4 / REGTEST), your address's live **balance** (from a public explorer), your hashrate/blocks, and live network cards (miners online + network hashrate). Run it in a real terminal (it's a full-screen TUI).

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
- **N-GPU + CPU grinding**: spawns one persistent grinder per GPU (`gpu_grind` OpenCL on Linux, `metal_grind` Metal on macOS/Apple Silicon). The kernel is compiled **once**; the nonce space is split across GPUs (and CPU) proportionally to their speed, so every device stays busy. Each candidate is **verified on the CPU** with a reference BLAKE2b before being submitted (the miner never trusts the GPU blindly).
- **Non-custodial payout**: the pool builds a coinbase that pays your address directly (99.1%) plus a 0.9% PyBLØCK fee output. The pool never holds your rewards.

## Files

```
src/main.rs        the miner (Rust + ratatui TUI: tabs, live stratum switching, native address gen, saved config)
gpu/gpu_grind.c    OpenCL host (Linux): search_b2b kernel driver, oneshot + persistent daemon modes
gpu/blake2b.cl     BLAKE2b-256 OpenCL kernel
gpu/metal_grind.m  Metal host (macOS / Apple Silicon): same daemon protocol as gpu_grind
gpu/blake2b.metal  BLAKE2b-256 Metal compute shader
tools/genaddr.py   legacy Python address generator (superseded by --genaddr; kept for reference)
build.sh           builds everything (Linux OpenCL / macOS Metal, CPU fallback)
```

Config is saved at `~/.config/pyblockminer/config.json` (stratums, selected pool, per-network addresses, donation, devices).

## License

MIT — see [LICENSE](LICENSE).
