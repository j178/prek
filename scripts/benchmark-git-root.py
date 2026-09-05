# /// script
# requires-python = ">=3.11"
# ///

"""Compare release binaries on fresh-process, warm-cache `prek list` startup.

Build both binaries with identical toolchains and release flags. The fixtures use
only a builtin hook, so listing requires no hook downloads or execution. Results
measure total command latency, including the process launcher, not just root().
"""

import argparse
import hashlib
import json
import os
import platform
import shutil
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


def run(command, cwd, env):
    start = time.perf_counter_ns()
    result = subprocess.run(command, cwd=cwd, env=env, capture_output=True, check=True)
    elapsed_ms = (time.perf_counter_ns() - start) / 1_000_000
    if result.stderr:
        raise RuntimeError(result.stderr.decode(errors="replace"))
    return result.stdout, elapsed_ms


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", type=Path, required=True)
    parser.add_argument("--head", type=Path, required=True)
    parser.add_argument("--runs", type=int, default=50)
    parser.add_argument("--warmup", type=int, default=5)
    parser.add_argument("--output", type=Path, help="Save metadata and all timing samples as JSON")
    args = parser.parse_args()
    if args.runs < 2 or args.warmup < 1:
        parser.error("require --runs >= 2 and --warmup >= 1")
    binaries = {"base": args.base.resolve(), "head": args.head.resolve()}
    for binary in binaries.values():
        if not binary.is_file() or not os.access(binary, os.X_OK):
            parser.error(f"not an executable file: {binary}")
    if binaries["base"].samefile(binaries["head"]):
        parser.error("provide distinct baseline and modified binaries")
    git = shutil.which("git")
    if git is None:
        parser.error("git is required to prepare the fixtures")
    env = {
        key: value for key, value in os.environ.items()
        if not key.startswith(("GIT_", "PREK_", "PRE_COMMIT_"))
        and key not in ("RUST_LOG", "RUST_BACKTRACE")
    }
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
               GIT_CONFIG_SYSTEM=os.devnull, NO_COLOR="1", TERM="dumb")
    report = {
        "platform": platform.platform(),
        "command": "prek --no-progress --no-log-file --color=never list",
        "cache": "warm; a fresh process for each sample",
        "order": "alternating base/head and head/base",
        "runs": args.runs,
        "warmup": args.warmup,
        "binaries": {
            label: {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
            for label, path in binaries.items()
        },
        "results": [],
    }
    with tempfile.TemporaryDirectory(prefix="prek-root-bench-") as directory:
        root = Path(directory)
        repo = root / "repo"
        run([git, "init", "-q", str(repo)], root, env)
        (repo / ".pre-commit-config.yaml").write_text(
            "repos:\n  - repo: builtin\n    hooks:\n      - id: trailing-whitespace\n"
        )
        run([git, "add", "."], repo, env)
        run([git, "-c", "user.name=Benchmark", "-c", "user.email=bench@example.com",
             "commit", "-qm", "fixture"], repo, env)
        nested = repo / "one/two/three/four/five"
        nested.mkdir(parents=True)
        linked = root / "linked"
        run([git, "worktree", "add", "-q", "--detach", str(linked)], repo, env)
        for case, cwd in (("root", repo), ("nested", nested), ("worktree", linked)):
            environments = {
                label: {**env, "PREK_HOME": str(root / f"{case}-{label}-home")}
                for label in binaries
            }
            commands = {
                label: [str(binary), "--no-progress", "--no-log-file", "--color=never", "list"]
                for label, binary in binaries.items()
            }
            samples = {label: [] for label in binaries}
            for iteration in range(args.warmup + args.runs):
                outputs = {}
                order = ("base", "head") if iteration % 2 == 0 else ("head", "base")
                for label in order:
                    outputs[label], elapsed = run(commands[label], cwd, environments[label])
                    if iteration >= args.warmup:
                        samples[label].append(elapsed)
                if outputs["base"] != outputs["head"] or not outputs["base"].strip():
                    raise RuntimeError(f"{case}: hook listings differ or are empty")
            base, head = (statistics.median(samples[label]) for label in ("base", "head"))
            reduction = 100 * (base - head) / base
            print(f"{case}: {base:.3f} -> {head:.3f} ms; "
                  f"saved {base - head:.3f} ms ({reduction:.1f}%)")
            report["results"].append({
                "case": case, "base_median_ms": base, "head_median_ms": head,
                "time_reduction_percent": reduction, "samples_ms": samples,
            })
    if args.output:
        args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
