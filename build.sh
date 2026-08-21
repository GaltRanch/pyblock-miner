#!/bin/bash
set -e
cd "$(dirname "$0")"
# preflight: OpenCL dev headers + ICD loader (the usual missing piece — lots of systems have the
# NVIDIA runtime but not the headers → 'CL/cl.h: No such file or directory').
if ! echo '#include <CL/cl.h>' | gcc -fsyntax-only -xc - 2>/dev/null; then
  echo "✗ OpenCL headers not found (CL/cl.h). Install the OpenCL dev headers + ICD loader:"
  echo "    Debian/Ubuntu:  sudo apt install ocl-icd-opencl-dev opencl-headers"
  echo "    Fedora:         sudo dnf install ocl-icd-devel opencl-headers"
  echo "    Arch:           sudo pacman -S opencl-icd-loader opencl-headers"
  exit 1
fi
echo "building gpu_grind (OpenCL)…"
gcc -O2 -DCL_TARGET_OPENCL_VERSION=120 gpu/gpu_grind.c -o gpu/gpu_grind -lOpenCL
echo "building pyblockMiner…"
cargo build --release
echo
echo "done."
echo "  1) generate an address:  python3 tools/genaddr.py"
echo "  2) mine:                 ./target/release/pyblockMiner --addr <your_address>"
