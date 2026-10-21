# Release PGO

Release builds use PGO for native x86_64 and ARM64 targets on Linux GNU and Windows,
and for ARM64 on macOS. Training covers core `run` paths and builtin hooks, followed
by a lightweight smoke test of the optimized binary.

Smoke-test an existing native release binary:

```bash
uv run scripts/pgo/train.py --verify /absolute/path/to/prek
```
