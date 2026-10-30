#!/usr/bin/env sh
# Shared by release runners, maturin containers, and Docker builds.
set -eu

# renovate: datasource=crate depName=cargo-auditable
CARGO_AUDITABLE_VERSION=0.7.6
# renovate: datasource=crate depName=rust-audit-info
RUST_AUDIT_INFO_VERSION=0.5.4

if [ "$#" -eq 0 ]; then
    echo "Usage: $0 <cargo-auditable|rust-audit-info>..." >&2
    exit 1
fi

for tool in "$@"; do
    case "$tool" in
        cargo-auditable) cargo install cargo-auditable --locked --version "$CARGO_AUDITABLE_VERSION" ;;
        rust-audit-info) cargo install rust-audit-info --locked --version "$RUST_AUDIT_INFO_VERSION" ;;
        *) echo "Unknown release tool: $tool" >&2; exit 1 ;;
    esac
done
