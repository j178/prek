# Release PGO

PGO is disabled by default and supports native x86_64 and ARM64 releases on
Linux GNU, macOS, and Windows. Training covers core `run` paths and builtin hooks;
release binaries receive a lightweight smoke test.

## Evaluation and rollout

Run [Evaluate PGO](../../.github/workflows/pgo.yml) to compare baseline and PGO builds
of the same commit on three independent runners per target. Review timings and
binary sizes in the Actions summaries, and build durations in the job logs.

Before enabling PGO for a target, require consistent results across all three runs:

- At least 5% lower geometric mean workload time
- No repeatable regression above 2% in any workload
- Identical behavior
- Acceptable binary size and build cost

Set the repository Actions variable
`PREK_RELEASE_PGO_TARGETS` to a JSON array of approved targets, for example
`["x86_64-unknown-linux-gnu", "aarch64-apple-darwin"]`. Remove a target to disable its
PGO build; an unset variable or `[]` disables all targets.

## Local validation

Smoke-test an existing native release binary:

```bash
python3 scripts/pgo/train.py --verify /absolute/path/to/prek
```

With hyperfine installed, compare baseline and PGO binaries:

```bash
python3 scripts/pgo/benchmark.py \
  --baseline /absolute/path/to/baseline \
  --pgo /absolute/path/to/pgo \
  --output results.json
```
