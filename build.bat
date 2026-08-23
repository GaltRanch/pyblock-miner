@echo off
REM pyblockMiner build - Windows.
REM   Builds the Rust miner (target\release\pyblockMiner.exe) and tries to build the OpenCL
REM   GPU grinder (gpu\gpu_grind.exe). Falls back to CPU-only if no OpenCL toolchain is found.
REM
REM   For GPU mining you need an OpenCL SDK + a C compiler, either:
REM     - MinGW-w64 gcc that can link -lOpenCL
REM         (MSYS2: pacman -S mingw-w64-x86_64-opencl-headers mingw-w64-x86_64-opencl-icd), OR
REM     - MSVC (Build Tools) + NVIDIA CUDA Toolkit (defines CUDA_PATH and ships OpenCL.lib).
setlocal
cd /d "%~dp0"

set GPU=0

REM 1) OpenCL GPU grinder - try MinGW gcc first
where gcc >nul 2>nul
if %errorlevel%==0 (
  echo building gpu_grind.exe with gcc ^(MinGW / OpenCL^)...
  gcc -O2 -DCL_TARGET_OPENCL_VERSION=120 gpu\gpu_grind.c -o gpu\gpu_grind.exe -lOpenCL && set GPU=1
)

REM ...else MSVC cl.exe + the CUDA OpenCL SDK
if "%GPU%"=="0" if defined CUDA_PATH (
  where cl >nul 2>nul
  if %errorlevel%==0 (
    echo building gpu_grind.exe with MSVC + CUDA OpenCL SDK...
    cl /nologo /O2 /DCL_TARGET_OPENCL_VERSION=120 gpu\gpu_grind.c /Fe:gpu\gpu_grind.exe /I "%CUDA_PATH%\include" "%CUDA_PATH%\lib\x64\OpenCL.lib" && set GPU=1
  )
)

if "%GPU%"=="0" (
  echo.
  echo [!] Could not build gpu_grind.exe - the miner will run CPU-only.
  echo     Install an OpenCL SDK + C compiler ^(MinGW-w64, or MSVC + CUDA Toolkit^) and re-run build.bat.
)

REM 2) Rust miner
echo building pyblockMiner ^(Rust^)...
cargo build --release
if %errorlevel% neq 0 (
  echo [x] cargo build failed - install Rust from https://rustup.rs
  exit /b 1
)

echo.
if "%GPU%"=="1" (echo done ^(GPU + CPU^).) else (echo done ^(CPU-only - no OpenCL^).)
echo   1^) address:  target\release\pyblockMiner.exe --genaddr testnet4
echo   2^) mine:     target\release\pyblockMiner.exe --network testnet4 --addr ^<tb1...^> --pool ^<host:port^>
endlocal
