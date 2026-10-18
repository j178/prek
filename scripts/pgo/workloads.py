"""Deterministic, offline repositories for PGO training and smoke tests."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path


def environment(root: Path) -> dict[str, str]:
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("PREK_", "PRE_COMMIT_", "GIT_", "XDG_"))
        and key not in {"SKIP", "CLICOLOR_FORCE", "FORCE_COLOR"}
    }

    env.update(
        PREK_HOME=str(root / "cache"),
        PREK_INTERNAL__USER_CONFIG_PATH=str(root / "user-config.toml"),
        XDG_CONFIG_HOME=str(root / "config"),
        GIT_CONFIG_NOSYSTEM="1",
        GIT_CONFIG_GLOBAL=os.devnull,
        GIT_TERMINAL_PROMPT="0",
        GIT_AUTHOR_NAME="PGO",
        GIT_AUTHOR_EMAIL="pgo@example.invalid",
        GIT_COMMITTER_NAME="PGO",
        GIT_COMMITTER_EMAIL="pgo@example.invalid",
        GIT_AUTHOR_DATE="2025-01-01T00:00:00Z",
        GIT_COMMITTER_DATE="2025-01-01T00:00:00Z",
        NO_COLOR="1",
        TERM="dumb",
    )
    return env


class Repository:
    def __init__(self, root: Path, env: dict[str, str]) -> None:
        root.mkdir()

        # Windows TEMP can use short names, while Git returns canonical paths.
        self.root = root.resolve()
        self.env = env

        self.git("init", "--quiet", "--template=", "--initial-branch=main")
        self.git("config", "core.autocrlf", "false")
        self.git("config", "commit.gpgsign", "false")

    def command(
        self, args: list[str | Path], expected: int = 0, *, input: bytes | None = None
    ) -> bytes:
        result = subprocess.run(
            args,
            input=input,
            cwd=self.root,
            env=self.env,
            capture_output=True,
            timeout=120,
            check=False,
        )

        if result.returncode != expected:
            raise RuntimeError(
                f"{args} exited {result.returncode}, expected {expected}:\n"
                + (result.stdout + result.stderr).decode(errors="replace")
            )

        return result.stdout + result.stderr

    def git(self, *args: str) -> bytes:
        return self.command(["git", *args])

    def write(self, name: str, content: str | bytes) -> None:
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content.encode() if isinstance(content, str) else content)

    def commit(self) -> None:
        self.git("add", "--all")
        self.git("-c", "core.hooksPath=", "commit", "--quiet", "-m", "Fixture")

    def reset(self) -> None:
        self.git("reset", "--quiet", "--hard", "HEAD")

    def run(self, binary: Path, *args: str, expected: int = 0) -> bytes:
        return self.command([binary, "--no-progress", *args], expected)

    def state(self) -> tuple[bytes, bytes, bytes]:
        return (
            self.git("status", "--porcelain"),
            self.git("diff", "--binary"),
            self.git("diff", "--cached", "--binary"),
        )


def config(*, toml: bool = False, system: bool = False) -> tuple[str, str]:
    hooks = [
        "trailing-whitespace",
        "end-of-file-fixer",
        "mixed-line-ending",
        "fix-byte-order-marker",
        "check-json",
        "check-jsonc",
        "check-json5",
        "check-yaml",
        "check-toml",
        "check-xml",
        "check-case-conflict",
        "check-merge-conflict",
        "detect-private-key",
        "check-executables-have-shebangs",
        "check-shebang-scripts-are-executable",
        "check-symlinks",
    ]

    if toml:
        content = '[[repos]]\nrepo = "builtin"\nhooks = [\n'
        content += "".join(
            f'  {{ id = "{hook}", priority = {min(i, 2)} }},\n'
            for i, hook in enumerate(hooks)
        )
        return "prek.toml", content + "]\n"

    content = "repos:\n  - repo: builtin\n    hooks:\n"
    content += "".join(f"      - id: {hook}\n" for hook in hooks)
    if system:
        content += """  - repo: local
    hooks:
      - id: noop
        name: noop
        language: system
        entry: git --version
        pass_filenames: false
"""
    return ".pre-commit-config.yaml", content


def populate(repo: Repository, count: int, *, prefix: str = "src") -> None:
    executables = []
    for i in range(count):
        directory = f"{prefix}/package-{i % 11}/nested"
        suffix, content = (
            ("txt", f"Document {i}\nSecond line\n"),
            ("json", f'{{"index": {i}, "items": [true, null, "text"]}}\n'),
            ("jsonc", f'{{\n  // Package settings\n  "index": {i}\n}}\n'),
            ("json5", f"{{index: {i}, items: ['one', 'two',],}}\n"),
            ("yaml", f"index: {i}\nitems: [one, two]\n"),
            ("toml", f'[package]\nname = "fixture-{i}"\n'),
            ("xml", f'<root><item id="{i}">text</item></root>\n'),
            ("", "#!/bin/sh\nexit 0\n"),
            ("bin", b"\x00\xff\x01binary\n"),
        )[i % 9]
        name = f"{directory}/file-{i}" + (f".{suffix}" if suffix else "")
        repo.write(name, content)
        if not suffix:
            (repo.root / name).chmod(0o755)
            executables.append(name)

    # Windows does not track executable bits in the filesystem.
    if executables:
        repo.git("add", "--chmod=+x", "--", *executables)

    repo.write("change.txt", "original\n")
    repo.write("invalid.json", "{}\n")

    # Symlink creation on Windows requires privileges or Developer Mode.
    if os.name != "nt":
        (repo.root / prefix / "linked.txt").symlink_to("../change.txt")


def fixture(
    root: Path, env: dict[str, str], count: int, *, workspace: bool = False
) -> Repository:
    repo = Repository(root, env)
    repo.write(*config(system=not workspace))
    populate(repo, count)

    if workspace:
        name, content = config(toml=True)
        repo.write(f"src/{name}", content)
        repo.write("other/.pre-commit-config.yaml", config()[1])
        populate(repo, count // 3, prefix="other")

    repo.commit()

    repo.write("change.txt", "committed change\n")
    repo.commit()
    return repo
