# Release PGO

PGO is disabled by default and currently supports only `x86_64-unknown-linux-gnu`
releases. Training covers core `run` paths (staged files, selection, workspaces,
and Git hook shims) and builtin parsing, checks, and fixes in temporary repositories
without downloads. Each PGO build generates fresh profiles. The final release binary
gets a small smoke test covering builtin checks, fixes, and staged-file recovery.

Like [uv](https://github.com/astral-sh/uv/blob/08c394201df4d77d39c03c3b8a1b36090a7b1c9a/scripts/build_uv_pgo.py),
training runs the Cargo binary directly. Maturin then packages the profile-guided
build. Training needs Python and Git, without a virtual environment or uv install.

## Evaluation and rollout

Manually run [Evaluate PGO](../../.github/workflows/pgo.yml) to compare baseline and PGO
builds of the same commit using the release configuration. Three independent
runners check each workload's observable behavior once, then measure the pair with
warm caches.
Review raw timings and binary sizes in `pgo-results-*`, and compare build durations
in the Actions logs.

Before enabling PGO, require consistent results across all three runs:

- At least 5% lower geometric mean workload time
- No repeatable regression above 2% in any workload
- Identical behavior, including fixes and staged-file recovery
- Acceptable binary size and build cost

Inspect sample variation before deciding. Set the repository Actions variable
`PREK_RELEASE_PGO=true` to enable PGO. Remove it or set it to `false` to disable it.
Build and verification failures fail the job without falling back to an ordinary
build. Other targets and Docker are unaffected.

## Local validation

Smoke-test an existing native release binary:

```bash
python3 scripts/pgo/train.py --binary /absolute/path/to/prek
```

Build with the pinned Rust toolchain and its LLVM tools. The C/C++ flags avoid
[incompatible profiling runtimes](https://github.com/rust-lang/cc-rs/issues/1986).
Linked worktrees share the primary checkout's Cargo target directory:

```bash
export CARGO_TARGET_DIR="$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")/target"
rustup component add llvm-tools-preview
target=$(rustc -vV | sed -n 's/^host: //p')
export CFLAGS="${CFLAGS:+$CFLAGS }-fno-profile-generate -fno-profile-use"
export CXXFLAGS="${CXXFLAGS:+$CXXFLAGS }-fno-profile-generate -fno-profile-use"
python3 scripts/pgo/train.py --target "$target"
RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-Cprofile-use=$CARGO_TARGET_DIR/pgo/prek.profdata" \
uvx --from maturin==1.15.0 maturin build \
  --release --locked --features self-update --target "$target" --out dist
```

Install hyperfine 2.0.0 and preserve a baseline built without `-Cprofile-use`, then compare:

```bash
python3 scripts/pgo/benchmark.py \
  --baseline /absolute/path/to/baseline \
  --pgo /absolute/path/to/pgo \
  --output results.json
```

Hyperfine measures baseline then PGO for each workload, with five warmups and forty
samples per binary by default. Samples determine the median and p95; repository
preparation runs outside timing. `--warmup 0 --runs 2` checks the harness only.
Raw profiles stay in `$CARGO_TARGET_DIR/pgo/profiles` for debugging and are cleared
before training.
