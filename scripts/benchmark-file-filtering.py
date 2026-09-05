# /// script
# requires-python = ">=3.11"
# ///

"""Compare two release binaries on warm, file-filtering workloads.

Build both binaries with the same toolchain and release flags. This script runs
real hooks, checks their dry-run file lists first, and alternates command order.
The unfiltered case is a control; improvements are reported per workload.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


CASES = ("unfiltered", "project-exclude", "selected-hook", "selected-project")
CONFIG_NAME = ".pre-commit-config.yaml"
CONFIG = """exclude: '(^|/)\\.pre-commit-config\\.yaml$'
repos:
  - repo: builtin
    hooks:
      - id: trailing-whitespace
"""


def checked_run(command: list[str], cwd: Path, env: dict[str, str]) -> bytes:
    result = subprocess.run(command, cwd=cwd, env=env, capture_output=True, check=True)
    if result.stderr:
        raise RuntimeError(result.stderr.decode(errors="replace"))
    return result.stdout


def create_fixture(root: Path, case: str, files: int, selected: int) -> tuple[Path, list[str], int]:
    repo = root / case / "repo"
    repo.mkdir(parents=True)
    for directory, count in (("src", selected), ("vendor", files - selected)):
        target = repo / directory
        target.mkdir()
        for index in range(count):
            (target / f"file{index:06}.txt").write_text("clean line\n" * 16)

    config = CONFIG
    command = ["run", "--all-files", "--color=never"]
    expected = selected
    if case == "unfiltered":
        expected = files
    elif case == "project-exclude":
        config = config.replace("(^|/)", "^vendor/|(^|/)", 1)
    elif case == "selected-hook":
        config += "        files: ^src/\n      - id: end-of-file-fixer\n"
        command.append("trailing-whitespace")
    elif case == "selected-project":
        (repo / "src" / CONFIG_NAME).write_text("orphan: true\n" + CONFIG)
        command.append("src/")
    else:
        raise ValueError(case)
    (repo / CONFIG_NAME).write_text(config)

    git = ["git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false"]
    subprocess.run([*git, "init", "-q"], cwd=repo, check=True)
    subprocess.run([*git, "add", "."], cwd=repo, check=True)
    subprocess.run(
        [*git, "-c", "user.name=Benchmark", "-c", "user.email=bench@prek.dev",
         "commit", "-qm", "Create benchmark fixture"],
        cwd=repo,
        check=True,
    )
    return repo, command, expected


def normalize(output: bytes) -> bytes:
    return re.sub(rb"(?m)^\s*- duration:.*$", b"", output)


def summary(samples: list[float]) -> dict[str, float]:
    return {
        "mean_seconds": statistics.mean(samples),
        "median_seconds": statistics.median(samples),
        "stdev_seconds": statistics.stdev(samples),
        "min_seconds": min(samples),
        "max_seconds": max(samples),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", type=Path, required=True, help="Baseline release binary")
    parser.add_argument("--head", type=Path, required=True, help="Modified release binary")
    parser.add_argument("--files", type=int, default=10000)
    parser.add_argument("--selected", type=int, default=100)
    parser.add_argument("--runs", type=int, default=30)
    parser.add_argument("--warmup", type=int, default=5)
    parser.add_argument("--case", choices=CASES, action="append")
    parser.add_argument("--output", type=Path, help="Save metadata and every timing sample as JSON")
    args = parser.parse_args()
    if not 0 < args.selected <= args.files:
        parser.error("require 0 < --selected <= --files")
    if args.runs < 2 or args.warmup < 1:
        parser.error("require --runs >= 2 and --warmup >= 1")

    binaries = {"base": args.base.resolve(), "head": args.head.resolve()}
    for path in binaries.values():
        if not path.is_file() or not os.access(path, os.X_OK):
            parser.error(f"not an executable file: {path}")
    report = {
        "platform": platform.platform(),
        "cpu_count": os.cpu_count(),
        "files": args.files,
        "selected_files": args.selected,
        "runs": args.runs,
        "warmup": args.warmup,
        "cache": "warm",
        "order": "alternating base/head and head/base",
        "binaries": {
            label: {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
            for label, path in binaries.items()
        },
        "results": [],
    }
    env = os.environ.copy()
    env.update({"NO_COLOR": "1", "TERM": "dumb", "GIT_CONFIG_GLOBAL": os.devnull,
                "GIT_CONFIG_NOSYSTEM": "1", "GIT_TERMINAL_PROMPT": "0"})
    report["concurrency_environment"] = {
        key: value for key, value in env.items()
        if key.startswith("RAYON_") or (key.startswith("PREK_") and "CONCURRENCY" in key)
    }

    with tempfile.TemporaryDirectory(prefix="prek-filter-bench-") as directory:
        root = Path(directory)
        for case in args.case or CASES:
            repo, command, expected = create_fixture(root, case, args.files, args.selected)
            environments = {
                label: {**env, "PREK_HOME": str(root / case / f"{label}-home")}
                for label in binaries
            }
            for label, path in binaries.items():
                report["binaries"][label]["version"] = checked_run(
                    [str(path), "--version"], repo, environments[label]
                ).decode().strip()

            plans = {}
            for label, path in binaries.items():
                plan_env = {**environments[label], "PREK_INTERNAL__SORT_FILENAMES": "1"}
                plans[label] = normalize(checked_run(
                    [str(path), *command, "--dry-run", "--verbose"], repo, plan_env
                ))
                counts = re.findall(rb"would be run on (\d+) files:", plans[label])
                if counts != [str(expected).encode()]:
                    raise RuntimeError(f"{case}: unexpected file counts for {label}: {counts}")
            if plans["base"] != plans["head"]:
                raise RuntimeError(f"{case}: base/head dry-run file lists differ")

            expected_output = None
            samples = {label: [] for label in binaries}
            for iteration in range(args.warmup + args.runs):
                order = ("base", "head") if iteration % 2 == 0 else ("head", "base")
                for label in order:
                    start = time.perf_counter()
                    output = checked_run(
                        [str(binaries[label]), *command], repo, environments[label]
                    )
                    elapsed = time.perf_counter() - start
                    if expected_output is None:
                        expected_output = output
                    if output != expected_output:
                        raise RuntimeError(f"{case}: hook output changed for {label}")
                    if iteration >= args.warmup:
                        samples[label].append(elapsed)

            summaries = {label: summary(values) for label, values in samples.items()}
            base_time = summaries["base"]["median_seconds"]
            head_time = summaries["head"]["median_seconds"]
            reduction = 100 * (1 - head_time / base_time)
            speedup = base_time / head_time
            report["results"].append({
                "case": case,
                "command": command,
                "expected_hook_files": expected,
                "summaries": summaries,
                "samples_seconds": samples,
                "median_time_reduction_percent": reduction,
                "median_speedup_ratio": speedup,
                "at_least_30_percent_less_time": reduction >= 30,
            })
            print(f"{case}: {base_time * 1000:.2f} -> {head_time * 1000:.2f} ms; "
                  f"{reduction:+.1f}% less time; {speedup:.2f}x", flush=True)
            if args.output:
                args.output.parent.mkdir(parents=True, exist_ok=True)
                args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
