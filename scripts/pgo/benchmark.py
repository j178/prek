"""Compare two release binaries on workloads not used for PGO training."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import platform
import re
import shlex
import statistics
import subprocess
import tempfile
from pathlib import Path

from workloads import Repository, config, environment, fixture


def prepare_command(state: str) -> str:
    commands = ["git reset --quiet --hard HEAD"]
    if state == "staged":
        commands.extend(
            [
                "printf '%s\\n' 'held-out staged content' > change.txt",
                "git add change.txt",
                "printf '%s\\n' 'held-out staged content' 'local addition' > change.txt",
            ]
        )
    elif state == "fix":
        commands.append("printf '%s\\n' 'held-out repair   ' > change.txt")
    return " && ".join(commands)


def benchmark(
    binaries: list[Path], source: Path, root: Path, warmup: int, runs: int
) -> dict:
    env = environment(root)
    small = fixture(root / "small", env, 231)
    large = fixture(root / "large", env, 3500)
    workspace = fixture(root / "workspace", env, 462, workspace=True)
    checkout = Repository(root / "source", env)
    # A real source tree is a holdout for file discovery and type filtering.
    archive = subprocess.run(
        ["git", "-C", source, "archive", "HEAD"],
        env=env,
        check=True,
        capture_output=True,
    ).stdout
    subprocess.run(["tar", "-xf", "-"], input=archive, cwd=checkout.root, check=True)
    (checkout.root / "prek.toml").unlink(missing_ok=True)
    checkout.write(*config())
    checkout.commit()

    cases = [
        ("staged", small, ["run"], "staged"),
        ("builtin-all", large, ["run", "--all-files"], "clean"),
        ("json-glob", large, ["run", "check-json", "--glob", "**/*.json"], "clean"),
        ("workspace", workspace, ["run", "--all-files"], "clean"),
        ("fix", small, ["run", "--files", "change.txt"], "fix"),
        ("source-discovery", checkout, ["run", "--dry-run", "--all-files"], "clean"),
    ]
    results = {}
    for name, repo, args, state in cases:
        # A relative script works with both Unix shells and Windows cmd.exe.
        repo.write(".git/pgo-prepare.sh", prepare_command(state) + "\n")
        prepare_args = ["sh", ".git/pgo-prepare.sh"]
        expected = 1 if state == "fix" else 0
        observations = []
        for binary in binaries:
            repo.command(prepare_args)
            output = repo.run(binary, *args, expected=expected)
            # Stash logs include a timestamp and PID for the same preserved diff.
            output = re.sub(
                rb"[/\\]patches[/\\]\d+-\d+\.patch", b"/patches/[patch].patch", output
            )
            observations.append((output, repo.state()))
        if observations[0] != observations[1]:
            raise RuntimeError(f"{name}: output or worktree/index changes differ")

        output_file = root / f"{name}.json"
        command = [
            "hyperfine",
            "--shell=none",
            "--metrics=time_wall_clock",
            f"--warmup={warmup}",
            f"--runs={runs}",
            f"--prepare={shlex.join(prepare_args)}",
            f"--export-json={output_file}",
        ]
        if expected == 1:
            command.append("--ignore-failure=1")
        for label, binary in zip(("baseline", "pgo"), binaries):
            command.extend(
                [
                    "--command-name",
                    label,
                    shlex.join([str(binary), "--no-progress", *args]),
                ]
            )
        subprocess.run(command, cwd=repo.root, env=env, check=True, timeout=120)
        batch = json.loads(output_file.read_text())
        samples: list[list[float]] = [[], []]
        for index, result in enumerate(batch["results"]):
            for sample in result["measurements"]:
                if sample["exit_code"] != expected:
                    raise RuntimeError(
                        f"{name}: exited {sample['exit_code']}, expected {expected}"
                    )
                samples[index].append(sample["time_wall_clock"]["value"])
        medians = [statistics.median(values) for values in samples]
        ratio = medians[1] / medians[0]
        results[name] = {
            "baseline_seconds": samples[0],
            "pgo_seconds": samples[1],
            "median_seconds": medians,
            "p95_seconds": [
                sorted(values)[math.ceil(0.95 * len(values)) - 1] for values in samples
            ],
            "pgo_over_baseline": ratio,
        }
        print(
            f"{name}: {medians[0] * 1000:.2f} -> {medians[1] * 1000:.2f} ms ({(ratio - 1) * 100:+.2f}%)",
            flush=True,
        )
    ratio = statistics.geometric_mean(
        case["pgo_over_baseline"] for case in results.values()
    )
    binary_metadata = [
        {
            "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "bytes": binary.stat().st_size,
        }
        for binary in binaries
    ]
    size_change = binary_metadata[1]["bytes"] - binary_metadata[0]["bytes"]
    return {
        "platform": platform.platform(),
        "processor": platform.processor(),
        "warmup": warmup,
        "runs": runs,
        "binaries": binary_metadata,
        "binary_size_change": {
            "bytes": size_change,
            "percent": size_change / binary_metadata[0]["bytes"] * 100,
        },
        "cases": results,
        "geometric_mean_ratio": ratio,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--pgo", type=Path, required=True)
    parser.add_argument("--source", type=Path, default=Path.cwd())
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--warmup", type=int, default=5)
    parser.add_argument(
        "--runs",
        type=int,
        default=40,
        help="Samples per binary",
    )
    args = parser.parse_args()
    if args.runs < 2 or args.warmup < 0:
        parser.error("runs must be at least 2; warmup must be nonnegative")
    with tempfile.TemporaryDirectory(prefix="prek-pgo-bench-") as directory:
        results = benchmark(
            [args.baseline.resolve(), args.pgo.resolve()],
            args.source.resolve(),
            Path(directory),
            args.warmup,
            args.runs,
        )
    args.output.write_text(json.dumps(results, indent=2) + "\n")
    print(f"Geometric mean ratio: {results['geometric_mean_ratio']:.4f}")
    print("Review all three independent runs before enabling release PGO.")


if __name__ == "__main__":
    main()
