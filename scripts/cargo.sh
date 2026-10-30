#!/usr/bin/env sh
# Used by maturin to embed dependency metadata in release binaries.
set -eu
exec cargo auditable "$@"
