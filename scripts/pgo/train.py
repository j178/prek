"""Build and train a native release binary, or run lightweight checks with --verify."""

from __future__ import annotations

import argparse
import os
import subprocess
import tempfile
from pathlib import Path

from workloads import environment, fixture


def train(binary: Path, root: Path, profile_dir: Path) -> None:
    env = environment(root)
    env["LLVM_PROFILE_FILE"] = str(profile_dir / "prek-%m.profraw")
    small = fixture(root / "small", env, 84)
    medium = fixture(root / "workspace", env, 840, workspace=True)

    small.run(binary, "validate-config", ".pre-commit-config.yaml")
    small.run(binary, "list")
    small.run(binary, "install", "--hook-type", "pre-commit")
    small.run(binary, "install", "--hook-type", "pre-push")
    small.run(binary)
    for _ in range(4):
        small.write("change.txt", "staged change\n")
        small.write("invalid.json", '{"staged": [true, null, 42]}\n')
        small.git("add", "change.txt", "invalid.json")
        small.write("change.txt", "staged change\nunstaged addition\n")
        before = small.state()
        small.run(binary)
        small.run(binary, "run")
        small.command(["sh", ".git/hooks/pre-commit"])
        if small.state() != before:
            raise RuntimeError("The staged run did not preserve the index and worktree")
        small.reset()

    for _ in range(2):
        small.run(binary, "run", "--all-files")
        small.run(binary, "run", "--files", "change.txt", "invalid.json")
        small.run(binary, "run", "--glob", "**/*.json")
        small.run(binary, "run", "--directory", "src/package-0")
        small.run(binary, "run", "--from-ref", "HEAD~1", "--to-ref", "HEAD")
        medium.run(binary, "run", "--all-files")
        medium.run(binary, "run", "--dry-run", "--all-files")

    old = small.git("rev-parse", "HEAD~1").decode().strip()
    new = small.git("rev-parse", "HEAD").decode().strip()
    # Drive the installed shim with the ref records Git supplies on stdin.
    subprocess.run(
        ["sh", ".git/hooks/pre-push", "origin", str(small.root)],
        input=f"refs/heads/main {new} refs/heads/main {old}\n".encode(),
        cwd=small.root,
        env=env,
        check=True,
        capture_output=True,
        timeout=120,
    )

    for original, fixed in [
        (
            b"needs fixing   \n" * 128 + b"last line",
            b"needs fixing\n" * 128 + b"last line\n",
        ),
        (b"first\r\nsecond\nthird\n" * 128, b"first\nsecond\nthird\n" * 128),
        (b"\xef\xbb\xbf" + b"content\n" * 2048, b"content\n" * 2048),
    ]:
        small.write("change.txt", original)
        small.run(binary, "run", "--files", "change.txt", expected=1)
        if (small.root / "change.txt").read_bytes() != fixed:
            raise RuntimeError(
                "The fixer workload did not produce the expected content"
            )
        small.run(binary, "run", "--files", "change.txt")
        small.reset()

    for suffix, content in [
        ("json", '{"invalid": }\n'),
        ("jsonc", '{"invalid": }\n'),
        ("json5", "{invalid: }\n"),
        ("yaml", "items: [\n"),
        ("toml", "[package\n"),
        ("xml", "<root>\n"),
    ]:
        name = f"invalid.{suffix}"
        small.write(name, content)
        small.run(binary, "run", f"check-{suffix}", "--files", name, expected=1)


def verify(binary: Path, root: Path) -> None:
    repo = fixture(root / "smoke", environment(root), 9)
    repo.run(binary, "run", "--all-files")

    repo.write("change.txt", "staged change\n")
    repo.git("add", "change.txt")
    repo.write("change.txt", "staged change\nunstaged addition\n")
    before = repo.state()
    repo.run(binary)
    if repo.state() != before:
        raise RuntimeError("The staged run did not preserve the index and worktree")
    repo.reset()

    repo.write("change.txt", "needs fixing   \nlast line")
    repo.run(binary, "run", "--files", "change.txt", expected=1)
    if (repo.root / "change.txt").read_bytes() != b"needs fixing\nlast line\n":
        raise RuntimeError("The fixer did not produce the expected content")

    repo.write("invalid.json", '{"invalid": }\n')
    repo.run(binary, "run", "check-json", "--files", "invalid.json", expected=1)


def build_profile(target: str | None) -> None:
    source = Path(__file__).resolve().parents[2]
    version = subprocess.check_output(["rustc", "-vV"], cwd=source, text=True)
    host = next(
        line.removeprefix("host: ")
        for line in version.splitlines()
        if line.startswith("host: ")
    )
    target = target or host
    if target != host:
        raise ValueError(
            f"PGO training requires the native target {host}, got {target}"
        )
    if "CARGO_ENCODED_RUSTFLAGS" in os.environ:
        raise ValueError(
            "Use RUSTFLAGS instead of CARGO_ENCODED_RUSTFLAGS for PGO builds"
        )
    sysroot = subprocess.check_output(
        ["rustc", "--print", "sysroot"], cwd=source, text=True
    ).strip()
    suffix = ".exe" if os.name == "nt" else ""
    profiler = (
        Path(sysroot) / "lib" / "rustlib" / host / "bin" / f"llvm-profdata{suffix}"
    )
    if not profiler.is_file():
        raise RuntimeError("Install llvm-tools-preview for the active Rust toolchain")

    target_dir = (source / os.environ.get("CARGO_TARGET_DIR", "target")).resolve()
    pgo_dir = target_dir / "pgo" / target
    profile_dir = pgo_dir / "profiles"
    instrumented = pgo_dir / "instrumented"
    merged = pgo_dir / "prek.profdata"
    profile_dir.mkdir(parents=True, exist_ok=True)
    merged.unlink(missing_ok=True)
    for profile in profile_dir.glob("prek-*.profraw"):
        profile.unlink()

    env = os.environ.copy()
    env["CARGO_INCREMENTAL"] = "0"
    env["CARGO_TARGET_DIR"] = str(instrumented)
    env["RUSTFLAGS"] = (
        f"{env.get('RUSTFLAGS', '')} -Cprofile-generate={profile_dir}".strip()
    )
    # Keep native dependencies out of Rust's profiling runtime.
    if os.name != "nt":
        for variable in ("CFLAGS", "CXXFLAGS"):
            env[variable] = (
                f"{env.get(variable, '')} -fno-profile-generate -fno-profile-use".strip()
            )
    print(f"Building instrumented prek for {target}", flush=True)
    subprocess.run(
        [
            "cargo",
            "build",
            "--package",
            "prek",
            "--bin",
            "prek",
            "--release",
            "--locked",
            "--features",
            "self-update",
            "--target",
            target,
        ],
        cwd=source,
        env=env,
        check=True,
    )
    binary = instrumented / target / "release" / f"prek{suffix}"
    print(f"Training {binary}", flush=True)
    with tempfile.TemporaryDirectory(prefix="prek-pgo-") as directory:
        train(binary, Path(directory), profile_dir)
    profiles = sorted(profile_dir.glob("prek-*.profraw"))
    if not profiles or any(profile.stat().st_size == 0 for profile in profiles):
        raise RuntimeError("Training did not produce complete profiles")
    subprocess.run([profiler, "merge", "-o", merged, *profiles], check=True)
    print(f"Merged {len(profiles)} profiles into {merged}", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--verify",
        type=Path,
        metavar="BINARY",
        help="Run lightweight checks on an existing release binary",
    )
    mode.add_argument(
        "--target", help="Native Rust target (defaults to the rustc host)"
    )
    args = parser.parse_args()
    if args.verify is None:
        build_profile(args.target)
    else:
        with tempfile.TemporaryDirectory(prefix="prek-pgo-") as directory:
            verify(args.verify.resolve(), Path(directory))
        print("Release binary smoke test passed", flush=True)


if __name__ == "__main__":
    main()
