#!/usr/bin/env bash
# Source this in the build environment so linker paths use its Rust toolchain.
# MSVC targets already enable ICF by default.

linker_flags=

case "$TARGET" in
    *-apple-darwin)
        linker_flags="-Clinker=rust-lld -Clinker-flavor=ld64.lld -Clink-arg=--icf=safe"
        ;;
    aarch64-pc-windows-msvc)
        # link.exe can corrupt ARM64 PGO profiles.
        # https://github.com/rust-lang/rust/issues/156675
        linker="$(cygpath -m "$(rustc --print sysroot)")/lib/rustlib/$TARGET/bin/rust-lld.exe"
        linker_flags="-Clinker=$linker -Clinker-flavor=lld-link"
        ;;
    *-linux-*)
        linker_bin="$(rustc --print sysroot)/lib/rustlib/$(rustc --print host-tuple)/bin"

        if [ "$TARGET" = powerpc64le-unknown-linux-gnu ]; then
            # GCC 4.8 cannot use -fuse-ld=lld, so expose LLD as ld through -B.
            lld_dir="$(mktemp -d)"
            ln -s "$linker_bin/rust-lld" "$lld_dir/ld"
            test -x "$lld_dir/ld"
            linker_flags="-Clink-arg=-B$lld_dir"
        else
            lld_dir="$linker_bin/gcc-ld"
            test -x "$lld_dir/ld.lld"
            linker_flags="-Clink-arg=-B$lld_dir -Clink-arg=-fuse-ld=lld"
        fi

        linker_flags="$linker_flags -Clink-arg=-Wl,--icf=all"
        ;;
esac

export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }$linker_flags"
