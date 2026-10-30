"""Check that each wheel or release archive contains an auditable prek binary.

Requires rust-audit-info on PATH. Missing binaries or unreadable dependency
metadata fail the check, including metadata lost during packaging or stripping.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import tarfile
import tempfile
import zipfile
from pathlib import Path


def check(archive: Path) -> None:
    print(f"Checking dependency metadata: {archive}", flush=True)
    with tempfile.TemporaryDirectory() as directory:
        binary = Path(directory) / "prek"
        if archive.suffix in {".whl", ".zip"}:
            with zipfile.ZipFile(archive) as source:
                members = [
                    name
                    for name in source.namelist()
                    if Path(name).name in {"prek", "prek.exe"}
                ]
                (member,) = members
                binary.write_bytes(source.read(member))
        else:
            with tarfile.open(archive) as source:
                members = [
                    member
                    for member in source.getmembers()
                    if member.isfile()
                    and Path(member.name).name in {"prek", "prek.exe"}
                ]
                (member,) = members
                with source.extractfile(member) as contents:
                    binary.write_bytes(contents.read())

        result = subprocess.run(
            ["rust-audit-info", str(binary)],
            check=True,
            stdout=subprocess.PIPE,
            text=True,
        )
        metadata = json.loads(result.stdout)
        if not any(
            package.get("root") and package["name"] == "prek"
            for package in metadata["packages"]
        ):
            raise ValueError(f"Missing prek root package in {archive}")
        print(f"Verified dependency metadata: {archive}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archives", nargs="+", type=Path)
    for archive in parser.parse_args().archives:
        check(archive)


if __name__ == "__main__":
    main()
