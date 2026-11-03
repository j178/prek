@echo off
REM Used by maturin to embed dependency metadata in release binaries.
cargo.exe auditable %*
