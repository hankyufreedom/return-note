#!/usr/bin/env bash
set -euo pipefail
# Local, isolated toolchain. Does not change the user's shell or global Rust setup.
toolchain="${RETURN_NOTE_TOOLCHAIN:-$HOME/.local/share/return-note-toolchain}"
export CARGO_HOME="$toolchain/cargo"
export RUSTUP_HOME="$toolchain/rustup"
export PATH="$toolchain/cargo/bin:$toolchain/llvm-mingw-20261104-ucrt-ubuntu-22.04-x86_64/bin:$PATH"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER=x86_64-w64-mingw32-clang
exec cargo "$@"
