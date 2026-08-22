#!/bin/bash
set -e
cd "$(dirname "$0")"

# pyblockMiner build — cross-platform.
#   Linux  : builds the OpenCL GPU grinder (gpu/gpu_grind) + the Rust miner. Falls back to
#            CPU-only if the OpenCL headers or libOpenCL are missing (instead of aborting).
#   macOS  : builds the Metal GPU grinder (gpu/metal_grind, Apple Silicon) + the Rust miner.
#            Falls back to CPU only if the Metal build fails. (OpenCL is Linux-only here.)

OS="$(uname -s)"

build_rust() {
  echo "building pyblockMiner (Rust)…"
  cargo build --release
}

# ── macOS (Apple Silicon): Metal GPU grinder + Rust miner (CPU fallback if Metal build fails) ──
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
  if ! gcc -O2 -DCL_TARGET_OPENCL_VERSION=120 gpu/gpu_grind.c -o gpu/gpu_grind -lOpenCL; then
    echo "  ⚠ OpenCL link failed (libOpenCL / ICD loader missing?) — building CPU-only."
    GPU=0
  fi
fi

build_rust
echo
if [ "$GPU" = 1 ]; then echo "done (GPU + CPU)."; else echo "done (CPU-only — no OpenCL)."; fi
echo "  1) address:  ./target/release/pyblockMiner --genaddr <mainnet|testnet4|regtest>"
echo "  2) mine:     ./target/release/pyblockMiner --network <net> --addr <address> --pool <host:port>"
