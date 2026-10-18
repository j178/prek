#!/usr/bin/env bash
# Source this with the build environment's Python so RUSTFLAGS also reaches maturin.

if [ "$TARGET" = aarch64-unknown-linux-gnu ]; then
    # GNU ld can fail on large instrumented binaries; use lld for both builds.
    lld_dir="$(rustc --print sysroot)/lib/rustlib/$TARGET/bin/gcc-ld"
    export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-Clink-arg=-B$lld_dir -Clink-arg=-fuse-ld=lld"
fi

if [ "$PGO_ENABLED" = true ]; then
    "$1" scripts/pgo/train.py --target "$TARGET"
    target_dir="${CARGO_TARGET_DIR:-${GITHUB_WORKSPACE:-$PWD}/target}"
    export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-Cprofile-use=$target_dir/pgo/$TARGET/prek.profdata"
fi
