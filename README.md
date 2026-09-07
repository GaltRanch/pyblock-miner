# pyblockMiner

**A GPU/CPU miner for PyBLØCK's Bitcoin BLAKE2b pools — LOTTO · CHIRP · CAROUSEL — live on mainnet, non-custodial.**

A terminal (TUI) miner: you mine to **your own** Bitcoin address and keep **99.1%** of every block you find, straight in your address (PyBLØCK pool fee 0.9%). No accounts, no custody. It saturates all your GPUs — NVIDIA, AMD or Intel (and/or CPU cores) — and shows live cards for your hashrate, blocks and difficulty — plus **live network cards**: how many miners are online and the network's total hashrate.

It's a **tabbed app** — everything is in the program, no restarts needed:

```
PyBLØCK  1·MINE  2·DATA  3·STRATUMS  4·LEARN  5·NETWORK  6·SETUP  7·HELP
╭ ⛏ Bitcoin BLAKE2b · syndicate · weighted split ─────────────────────────────────────────╮
│ 🌌 CHIRP BLAKE2b   MAINNET   ● LIVE   pool.pyblock.xyz:5574                             │
│ your address  bc1q…   balance 0.01234567 BTC   your slice of every block  0.84% · fee 0.9%│
╰──────────────────────────────────────────────────────────────────────────────────────────╯
╭ YOUR HASHRATE ──╮ ╭ BLOCKS FOUND ──╮ ╭ DIFFICULTY ──╮
│    26.5 GH/s    │ │       0        │ │    bits 2    │
╭ ◈ IN THE COINBASE ╮ ╭ ◈ SYNDICATE HASHRATE ╮ ╭ ◈ LAST BLOCK REWARD ╮
│        38         │ │      8.07 TH/s       │ │  3.3816 BTC · your cut ≈ 0.028 BTC │
╭ 🌌 CHIRP · who is in the coinbase · 58 miners · ↑↓ 1–18 of 58 ───────────────────────────╮
│   #  MINER                                       TENURE  POWER 24h  SHARE          STATUS │
│   1  bc1qjdqlvwfxum8dh4t5v9mvskdarjvlek2a9g5pw2   7.2 d      57.3M  ▰▰▰▰▰▰▰▰▰▰  30.00%  ● │
│ ▶ 2  bc1q…you…                                    7.2 d       1.6M  ▰▱▱▱▱▱▱▱▱▱   0.84%  ● you │
│  …   every eligible miner, then the ones still earning their 7 days (⏳ 3.1 d to go)      │
╰──────────────────────────────────────────────────────────────────────────────────────────╯
```

### Three pools, one chain — the MINE tab adapts to how each one pays

| pool | port | what the coinbase does | what MINE shows you |
|------|------|------------------------|---------------------|
| 🎰 **LOTTO** | `4445` | solo lottery — every block you find pays **your** address · you keep 99.1% · fee 0.9% | your odds: expected time-to-block at your hashrate |
| 🌌 **CHIRP** | `5574` | syndicate — every block is **split on-chain among ALL eligible miners** by weight (7-day loyalty) · the syndicate mines the **suppliers' clean templates** (CHIRP + CAROUSEL) · syndicate 98% · supplier 1% · PyBLØCK 1% | **everyone in the coinbase draw**: rank, address, tenure, power, share % and status, your row marked `▶ you`, your live slice + cut in BTC after fees, plus the **template being mined now** · `↑↓` scrolls |
| 🎠 **CAROUSEL** | `30110` | rotating **clean templates** from independent suppliers — finder keeps 96% · supplier 3% · PyBLØCK 1% | the template being mined **right now**, the whole rotation, the recent trail |
| ᛞ **WAVICLES** | *your gateway* · `b.pyblock.xyz:23114` (ASIC) | **bring your own node**: a Knots BLAKE2b node (≥ 29.4.1) + a DATUM gateway build the block; the pool (`b.pyblock.xyz:28915`, DATUM only, never for miners) dictates the split — **99.6% to the TIDES work window**, split by share of work · 0.4% fee · any block found by anyone in the window pays everyone in it | **the whole window**: every identity, share of work, what each would get if a block hit now, hashrate, last share, payable or not; your row marked `▶ you`, your expected BTC/day · `↑↓` scrolls |

Two WAVICLES entries ship in STRATUMS. **`WAVICLES · your node (DATUM)`**: select it, press `e` and type your gateway's stratum `host:port` (CONVOY ≥ b9ea7dc, OCEAN forks or StartOS; `vardiff_min 1` for GPUs). Miners log in to the gateway with your **address** (a worker suffix is fine) — that's the identity the window pays. **`WAVICLES · via PyBLØCK's node (ASIC)`** is the house gateway on `:23114`: no node needed, but its vardiff floor is 4096, so a GPU would not submit a share for hours. If a stratum hands out **SHA-256 work** (a regular pool, or a SHA-256 DATUM gateway on the same box) the miner refuses it — `⛔ SHA-256 WORK` in the header, an alert, no wasted power. CONVOY/OCEAN gateways send Sia-style fractional difficulties and put the *share* target in the job's nbits; the miner normalizes the former and takes the network difficulty from the pool API, so shares land and the ETA is honest.

The header, the network tiles and the panel all follow the selected stratum. **NETWORK** (`5`) shows the same mode panel full-height. Data comes from the pool's own APIs (`chirp_api.php?chain=blake2b`, `carousel.php?carrousel=1`), refreshed every 15 s.

- **STRATUMS** — pick a pool and **switch it live, without leaving the miner.** Each pool says in one line what it does with the coinbase. PyBLØCK's pools are there by default (LOTTO / CHIRP / CAROUSEL / testnet4 / regtest); add your own custom stratums too.
- **SETUP** — generate or paste your address, per network; toggle CPU and the donation. Everything **auto-saves** to a config file, so you don't re-type flags.
- **LEARN / HELP** — what BLAKE2b is, where the chain stands, and troubleshooting (incl. the OpenCL-headers fix).

> ✅ **Mainnet is live.** Bitcoin BLAKE2b — the proof-of-work change born in Bitcoin Knots [PR #359](https://github.com/bitcoinknots/bitcoin/pull/359) — is **running on mainnet and mining is working optimally.** As of September 2026 the chain is past block 968,000, PyBLØCK's pools have found **1,100+ blocks** (≈3.125 BTC each) and the network hashes at **≈1.2 PH/s** across 120+ miners, GPUs and ASICs alike. The miner's default stratums are the three **mainnet** pools (LOTTO · CHIRP · CAROUSEL); every block pays real BTC to **your** address. **testnet4** (`:23111`) and **regtest** (`:23110`) remain available for testing and carry no value. Each network needs its own address type (`bc1…` mainnet · `tb1…` testnet4 · `bcrt1…` regtest); the dev donation applies **only on mainnet**. Live numbers: [b.pyblock.xyz](https://b.pyblock.xyz:8443/). Don't trust, verify.
>
> **A word on odds.** With ASICs on the network, a single GPU on LOTTO is a true lottery (the DATA tab shows your expected time-to-block honestly). If you'd rather earn a steady, weighted share of every block the syndicate finds, mine on **CHIRP**.

---

## Download (no toolchain needed)

Prebuilt packages for every release are on the [Releases page](https://github.com/GaltRanch/pyblock-miner/releases): `linux-x86_64` (OpenCL), `macos-arm64` (Apple Silicon, Metal) and `windows-x86_64` (OpenCL). Unpack and run `pyblockMiner` — the `gpu/` folder next to it holds the grinder and kernel.

- **Linux** needs the OpenCL ICD loader at runtime (`libOpenCL.so.1` — comes with your GPU driver, or `apt install ocl-icd-libopencl1`).
- **macOS**: the binary is not notarized; the first run may need `xattr -dr com.apple.quarantine pyblockMiner-*/`.
- **Windows**: `gpu\OpenCL.dll` (the Khronos ICD loader) is bundled; your GPU driver provides the actual OpenCL.
- Verify downloads with `SHA256SUMS.txt`. Release binaries **update themselves**: press `u` twice (or `--update`) and the miner fetches the matching release, swaps itself in place and relaunches.

Building from source is fully supported too:

## Requirements

- **Rust** (`cargo`) — https://rustup.rs
- **GPU (optional — no GPU falls back to CPU):**
  - **Linux:** an OpenCL GPU (NVIDIA / AMD / Intel). Needs `gcc` + OpenCL headers + ICD loader to build the grinder:
    - Debian/Ubuntu: `sudo apt install ocl-icd-opencl-dev opencl-headers`
    - Fedora: `sudo dnf install ocl-icd-devel opencl-headers`
    - Arch: `sudo pacman -S opencl-icd-loader opencl-headers`
    - Missing headers/lib? `./build.sh` just builds CPU-only (no abort).
  - **macOS (Apple Silicon):** the Metal grinder builds with the Xcode command-line tools (`xcode-select --install`). No OpenCL needed.
  - **Windows:** an OpenCL GPU. Build the grinder (`gpu\gpu_grind.exe`) with **MSYS2/MinGW-w64** — install OpenCL with `pacman -S mingw-w64-x86_64-opencl-headers mingw-w64-x86_64-opencl-icd`, then run `build.bat` from the MinGW64 shell — **or** MSVC Build Tools + the NVIDIA CUDA Toolkit (defines `CUDA_PATH`, ships `OpenCL.lib`). No toolchain? `build.bat` just builds CPU-only. (The miner auto-appends `.exe` when locating the grinder on Windows.)
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
| `--cpu` | off | also mine on the CPU (added as an extra worker next to the GPUs; `--gpus 0` = CPU only) |
| `--donate <pct>` | `2.0` | PyBLØCK hashrate donation percent (mainnet only, minimum 2.0, see below) |
| `--worker <name>` | *(none)* | log in as `addr.name` so the pool tells your rigs apart (also SETUP → `w`) |
| `--api-port <N>` | off | local stats API: `http://127.0.0.1:N/` (JSON) and `/metrics` (Prometheus) — localhost only |
| `--telegram <token>,<chat_id>` | off | alerts to a Telegram chat (also SETUP → `t`) |
| `--webhook <url>` | off | alerts as a JSON `POST {source,title,body,ts}` |
| `--no-log-file` · `--no-bell` · `--no-desktop` | on | turn off the timestamped `miner.log`, the terminal bell, or desktop notifications |
| `--rune unicode` | `bowtie` | WAVICLES' mark is the Dagaz rune ᛞ; most terminal fonts lack the Runic block, so the miner shows the look-alike `⋈` by default. Pick `unicode` if your font has ᛞ. |
| `--gpu-pool 0=CHIRP,1=WAVICLES,2=my-pool` | *(all on the selected stratum)* | **multi-pool rig**: each GPU (and the CPU, as the last worker index) mines its own pool — one stratum session per group, its own vardiff, watchdogs and share count; unassigned workers mine the selected stratum. Also SETUP → `r`. `g` in the app cycles which group the header/tiles/panel show. |
| `--sweep-ms <ms>` · `--gpu-iter <n>` | adaptive · 512 | work-size knobs for mixed-speed rigs, flaky power or Windows TDR: cap each sweep's length (smaller nonce range per device) and/or the nonces per GPU work-item (smaller kernel launches: 512 = 2^31 nonces per launch, 128 = 2^29). Costs a little hashrate; try `--gpu-iter 128` first. Persist in config. |
| `--update` | | `git pull` + build in this checkout, then exit (same as pressing `u` twice in the app) |
| `--auto-update` | off | headless services: when the pool announces a newer version, pull + build + relaunch on their own |

### Multi-pool rigs — one GPU on CHIRP, another on WAVICLES, another wherever

Every worker can mine a different pool at the same time. Open the **RIG** tab (`8`, or `r` from SETUP): a grid of your workers × the pools of the current network. Move with the arrows, press **Enter** to send the whole worker to that pool, **`+` / `−`** to move 10% of its power onto or off that pool (so a GPU can mine CHIRP 70% / WAVICLES 30%), **`0`** to put it back on the selected stratum, **`a`** to send every worker to the pool under the cursor. Changes save at once and apply a moment after your last key; the grid shows each pool's live hashrate and accepted shares. From the shell the same thing is `--gpu-pool 0=CHIRP,1=WAVICLES:70+LOTTO:30`. Anything unassigned stays on the selected stratum, so with no assignments the miner behaves exactly as before. Each group is its own stratum session — connection, vardiff, backoff, job-freshness and dead-work watchdogs, SHA-256 refusal, share counts — a split worker alternates its sweeps between its pools in proportion, and the nonce space is split inside each group, never across pools.

With more than one group **MINE becomes a rig overview**: a GROUPS card with every pool, link state, hashrate, workers (and their split), difficulty, shares and what that pool pays you right now; `g` drills into a group (its own header, network tiles and panel — CHIRP's coinbase draw, WAVICLES' TIDES window, …) and back to the overview. The WORKERS card names every worker's pool(s), and the local API lists `groups` with per-pool hashrate and shares. Names match STRATUMS entries case-insensitively (`CHIRP` → `PyBLØCK · CHIRP`; the CPU is the last worker index).

### Updates

The pool announces the latest miner version. When yours is older you get an **alert** (bell / desktop / Telegram) and the header shows `⬆ v0.2.x available — press u to update`. Press **`u` twice** and the miner leaves the screen, runs `git pull` and the build in the checkout it was built from, and **relaunches itself** on the new version (mining pauses for the build, one to two minutes). From the shell: `pyblockMiner --update`. For a systemd/headless service, `--auto-update` does it unattended.

### Alerts, log file, local API

The miner tells you what matters even when you're not looking: **block found**, **GPU down / back online**, **pool unreachable / back**, and on CHIRP **you're on the list · you're in the coinbase draw · you're falling out (no shares for 1 h) · you dropped off**. Channels: terminal bell, desktop notification (`notify-send` on Linux, Notification Center on macOS), Telegram, webhook. Toggle them in **SETUP** (`b` bell · `n` desktop · `t` telegram · `x` send a test alert).

Every log line is also appended, timestamped, to `~/.config/pyblockminer/miner.log` (rotated at 5 MB) so a bad night can be reconstructed. With `--api-port 18080`, `curl 127.0.0.1:18080/` returns live JSON (hashrate, workers, shares, your CHIRP slice and expected BTC/day, balance…) and `/metrics` feeds Prometheus/Grafana.

### Keys

| key | action |
|-----|--------|
| `1`–`8` / `Tab` | switch tabs (MINE · DATA · STRATUMS · LEARN · NETWORK · SETUP · HELP · RIG) |
| RIG: `↑↓` `←→` `Enter` `+`/`−` `0` `a` `c` | worker · pool · whole worker here · move 10% of its power · back to the selected stratum · everyone here · clear all |
| `p` | pause / resume mining from any tab |
| MINE / NETWORK: `↑↓` `PgUp` `PgDn` `Home` `End` | scroll the CHIRP coinbase list (everyone in the draw) |
| `q` / `Esc` | quit (`Esc` also cancels a text input) |
| STRATUMS: `↑↓` `Enter` `e` `a` `d` | move · **switch live** · edit address (WAVICLES: your gateway) · add custom · delete custom |
| SETUP: `g` `e` `w` `c` `+/-` | generate address · edit/paste address · worker name · toggle CPU · donation |
| SETUP: `b` `n` `t` `x` `r` | bell · desktop notifications · Telegram `token,chat_id` · send a test alert · open the RIG tab |
| `g` | multi-pool rigs: next GPU group (header, tiles and panel follow it) |
| LEARN: `←` `→` | previous / next info page |

The **MINE** tab shows the pool mode (LOTTO / CHIRP / CAROUSEL), a network badge (MAINNET / TESTNET4 / REGTEST), your address's live **balance** (from the PyBLØCK BLAKE2b node), your hashrate/blocks, mode-aware network cards, and the coinbase panel described above. Run it in a real terminal (it's a full-screen TUI); it lays out for any width — addresses show in full on wide terminals and masked (`bc1qjd…5pw2`) on narrow ones.

**It looks after itself.** A GPU grinder that crashes or stops answering is killed and **respawned automatically** (backoff 10 s → 5 min; the WORKERS row shows `○ offline · auto-respawning` meanwhile). Pool connections have an 8 s timeout and reconnect with exponential backoff (3 s → 60 s), so a dead pool never looks like a hung miner. If the engine ever stops ticking while connected, the header flips to `⚠ ENGINE STALLED` instead of lying with a green LIVE.

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
