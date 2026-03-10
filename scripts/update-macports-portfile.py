# /// script
# requires-python = ">=3.14"
# dependencies = [
#     "httpx>=0.28.1",
# ]
# ///

from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

import httpx


def run(cmd: list[str], *, capture: bool = False) -> str:
    result = subprocess.run(
        cmd,
        check=True,
        text=True,
        capture_output=capture,
    )
    if capture:
        return result.stdout.strip()
    return ""


def repo_root() -> Path:
    root = run(["git", "rev-parse", "--show-toplevel"], capture=True)
    return Path(root)


def read_version(cargo_toml: Path) -> str:
    content = cargo_toml.read_text(encoding="utf-8")
    match = re.search(r'^version\s*=\s*"([^"]+)"', content, flags=re.MULTILINE)
    if not match:
        raise RuntimeError(f"Failed to read version from {cargo_toml}")
    return match.group(1)


def replace_github_setup_version(portfile_text: str, version: str) -> str:
    updated = re.sub(
        r'^(github\.setup\s+\S+\s+\S+\s+)\S+',
        rf'\g<1>{version}',
        portfile_text,
        count=1,
        flags=re.MULTILINE,
    )
    if updated == portfile_text:
        raise RuntimeError("Could not locate github.setup line in Portfile")
    return updated


def download_distfile(version: str) -> Path:
    distfile = Path(f"/tmp/prek-v{version}.tar.gz")
    url = f"https://github.com/j178/prek/archive/v{version}.tar.gz"
    with httpx.Client(follow_redirects=True, timeout=60.0) as client:
        response = client.get(url)
        response.raise_for_status()
        distfile.write_bytes(response.content)
    return distfile


def openssl_digest(algorithm: str, file_path: Path) -> str:
    out = run(["openssl", "dgst", f"-{algorithm}", str(file_path)], capture=True)
    if "= " not in out:
        raise RuntimeError(f"Unexpected openssl output: {out}")
    return out.split("= ", 1)[1].strip()


def update_checksums_block(portfile_text: str, rmd160: str, sha256: str, size: int) -> str:
    updated = re.sub(
        r"(^\s*rmd160\s+)\S+(\s*\\\s*$)",
        rf"\g<1>{rmd160}\g<2>",
        portfile_text,
        count=1,
        flags=re.MULTILINE,
    )
    updated = re.sub(
        r"(^\s*sha256\s+)\S+(\s*\\\s*$)",
        rf"\g<1>{sha256}\g<2>",
        updated,
        count=1,
        flags=re.MULTILINE,
    )
    updated = re.sub(
        r"(^\s*size\s+)\d+(\s*$)",
        rf"\g<1>{size}\g<2>",
        updated,
        count=1,
        flags=re.MULTILINE,
    )

    if updated == portfile_text or "rmd160" not in updated or "sha256" not in updated:
        raise RuntimeError("Could not locate checksum lines in Portfile")
    return updated


def ensure_cargo2port() -> None:
    if shutil.which("cargo2port"):
        return
    run(
        [
            "cargo",
            "install",
            "--locked",
            "--git",
            "https://github.com/l2dy/cargo2port",
            "cargo2port",
        ]
    )


def generated_cargo_crates(cargo_lock: Path) -> str:
    return run(["cargo2port", str(cargo_lock)], capture=True)


def replace_cargo_crates_block(portfile_text: str, crates_block: str) -> str:
    marker = "cargo.crates"
    idx = portfile_text.find(marker)
    if idx == -1:
        raise RuntimeError("Could not locate cargo.crates block in Portfile")
    prefix = portfile_text[:idx]
    return prefix + crates_block.rstrip() + "\n"


def main() -> None:
    root = repo_root()
    default_portfile = root / "scripts" / "macports" / "Portfile"
    portfile = Path(os.environ.get("PORTFILE", str(default_portfile)))

    if not portfile.is_file():
        raise RuntimeError(f"Portfile not found at {portfile}")

    version = read_version(root / "Cargo.toml")

    text = portfile.read_text(encoding="utf-8")
    text = replace_github_setup_version(text, version)

    distfile = download_distfile(version)
    rmd160 = openssl_digest("rmd160", distfile)
    sha256 = openssl_digest("sha256", distfile)
    size = distfile.stat().st_size

    text = update_checksums_block(text, rmd160, sha256, size)

    ensure_cargo2port()
    crates_block = generated_cargo_crates(root / "Cargo.lock")
    text = replace_cargo_crates_block(text, crates_block)

    portfile.write_text(text, encoding="utf-8")
    print(f"Updated {portfile} for version {version}")
    print("To open a PR with the updated Portfile, run:")
    print(f"  git clone --depth=1 --branch=main https://github.com/macports/macports-ports.git /tmp/macports-ports")
    print(f"  cp {portfile} /tmp/macports-ports/devel/prek/Portfile")
    print(f"  cd /tmp/macports-ports")
    print(f"  git add devel/prek/Portfile")
    print(f"  git commit -m 'prek: update to {version}'")
    print(f"  gh pr create --title 'prek: update to {version}'")


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        raise SystemExit(1)
