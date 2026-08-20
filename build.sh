#!/bin/bash
set -e
cd "$(dirname "$0")"
echo "building gpu_grind (OpenCL)…"
gcc -O2 -DCL_TARGET_OPENCL_VERSION=120 gpu/gpu_grind.c -o gpu/gpu_grind -lOpenCL
echo "building pyblockMiner…"
cargo build --release
echo
echo "done."
echo "  1) generate an address:  python3 tools/genaddr.py"
echo "  2) mine:                 ./target/release/pyblockMiner --addr <your_address>"
