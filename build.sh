#!/bin/bash
set -e
cd "$(dirname "$0")"

# pyblockMiner build — cross-platform.
#   Linux  : builds the OpenCL GPU grinder (gpu/gpu_grind) + the Rust miner. Falls back to
#            CPU-only if the OpenCL dev headers are missing (instead of aborting).
#   macOS  : builds CPU-only. GPU mining uses OpenCL, which Apple deprecated, so the miner
#            auto-detects no NVIDIA GPU and mines on all CPU cores (blake2b_simd / NEON).
#            A Metal GPU backend is future work.

OS="$(uname -s)"

build_rust() {
  echo "building pyblockMiner (Rust)…"
  cargo build --release
}

# ── macOS (Apple Silicon / Intel): CPU-only ──
if [ "$OS" = "Darwin" ]; then
  echo "macOS detected — building the Metal GPU grinder + the Rust miner (Apple Silicon)."
  if clang -O2 -fobjc-arc gpu/metal_grind.m -o gpu/metal_grind -framework Metal -framework Foundation 2>&1; then
    echo "  ✓ metal_grind (Metal) built — will use the Apple GPU."
  else
    echo "  ⚠ metal_grind build failed — the miner falls back to CPU automatically."
  fi
  build_rust
  echo
  echo "done."
  echo "  1) address:  ./target/release/pyblockMiner --genaddr testnet4"
  echo "  2) mine:     ./target/release/pyblockMiner --network testnet4 --addr <tb1…> --pool <host:port>"
  exit 0
fi

# ── Linux: try the OpenCL GPU grinder; fall back to CPU-only if headers absent ──
GPU=1
if ! echo '#include <CL/cl.h>' | gcc -fsyntax-only -xc - 2>/dev/null; then
  echo "⚠ OpenCL headers not found (CL/cl.h) — building CPU-only."
  echo "  For GPU mining, install the OpenCL dev headers + ICD loader and re-run ./build.sh:"
  echo "    Debian/Ubuntu:  sudo apt install ocl-icd-opencl-dev opencl-headers"
  echo "    Fedora:         sudo dnf install ocl-icd-devel opencl-headers"
  echo "    Arch:           sudo pacman -S opencl-icd-loader opencl-headers"
  GPU=0
fi

if [ "$GPU" = 1 ]; then
  echo "building gpu_grind (OpenCL)…"
  gcc -O2 -DCL_TARGET_OPENCL_VERSION=120 gpu/gpu_grind.c -o gpu/gpu_grind -lOpenCL
fi

build_rust
echo
if [ "$GPU" = 1 ]; then echo "done (GPU + CPU)."; else echo "done (CPU-only — no OpenCL)."; fi
echo "  1) generate an address:  python3 tools/genaddr.py"
echo "  2) mine:                 ./target/release/pyblockMiner --addr <your_address>"
