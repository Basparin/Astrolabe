#!/usr/bin/env bash
# Windows build environment for astrolabe (no upstream pinned toolchain bundle).
# Source this, then run cargo. Recipe recovered/verified 2026-07-24..25.
#   . ./build-env.sh && cargo build --release -p astrolabe-server
MINGW="/c/Users/Basparin/AppData/Local/Microsoft/WinGet/Packages/BrechtSanders.WinLibs.POSIX.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe/mingw64"
LLVM="/c/Program Files/LLVM"
# Windows-form spellings: clang (spawned by bindgen, not by this shell) cannot
# resolve the MSYS "/c/..." form.
MINGW_WIN="C:/Users/Basparin/AppData/Local/Microsoft/WinGet/Packages/BrechtSanders.WinLibs.POSIX.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe/mingw64"

export PATH="$MINGW/bin:$LLVM/bin:$PATH"
export MAKE="$MINGW/bin/mingw32-make.exe"
export CC="$MINGW/bin/gcc.exe"
export CXX="$MINGW/bin/g++.exe"
export AR="$MINGW/bin/ar.exe"
export LIBCLANG_PATH="C:/Program Files/LLVM/bin"
export C_INCLUDE_PATH="D:/astrolabe/.deps/zlib/include"
export LIBRARY_PATH="D:/astrolabe/.deps/zlib/lib"
# bindgen shlex-splits BINDGEN_EXTRA_CLANG_ARGS, so a path containing a space
# ("C:/Program Files/LLVM") MUST stay quoted inside the value. Unquoted, clang
# receives "C:/Program" and "Files/LLVM/..." as two arguments, silently loses the
# clang builtin include dir, and bindgen dies with a bare
#   fatal error: 'stdbool.h' file not found
# whose stated cause is unrelated to the real one.
export BINDGEN_EXTRA_CLANG_ARGS="--target=x86_64-pc-windows-gnu -isystem \"C:/Program Files/LLVM/lib/clang/22/include\" -isystem \"$MINGW_WIN/x86_64-w64-mingw32/include\""
