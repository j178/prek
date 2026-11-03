"""Resume package publication by skipping versions already in the registry."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path


def is_published(url: str) -> bool:
    request = urllib.request.Request(
        url, headers={"User-Agent": "prek-release (https://github.com/j178/prek)"}
    )
    try:
        with urllib.request.urlopen(request, timeout=30):
            return True
    except urllib.error.HTTPError as error:
        error.close()
        if error.code == 404:
            return False
        # An unavailable registry or failed authentication is not a missing version.
        raise


def publish_crates() -> None:
    metadata = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"], text=True
        )
    )
    command = ["cargo", "publish", "--registry", "crates-io"]
    for package in metadata["packages"]:
        if package["id"] not in metadata["workspace_members"]:
            continue

        name, version = package["name"], package["version"]
        if is_published(f"https://crates.io/api/v1/crates/{name}/{version}"):
            print(f"Skipping {name}@{version}: already published", flush=True)
        else:
            command.extend(["--package", name])

    if "--package" not in command:
        print("All workspace crates are already published.")
        return

    # Cargo orders the remaining packages by their dependencies and waits for the index.
    subprocess.run(command, check=True)


def publish_npm(manifest: Path, tag: str) -> None:
    for directory in json.loads(manifest.read_text())["publishOrder"]:
        package = json.loads((Path(directory) / "package.json").read_text())
        name, version = package["name"], package["version"]
        encoded_name = urllib.parse.quote(name, safe="")
        if is_published(f"https://registry.npmjs.org/{encoded_name}/{version}"):
            print(f"Skipping {name}@{version}: already published", flush=True)
            continue

        subprocess.run(
            [
                "npm",
                "publish",
                "--registry",
                "https://registry.npmjs.org",
                "--tag",
                tag,
                "--provenance",
                "--access",
                "public",
                directory,
            ],
            check=True,
        )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="registry", required=True)
    commands.add_parser("crates")
    npm = commands.add_parser("npm")
    npm.add_argument("--manifest", type=Path, required=True)
    npm.add_argument("--tag", default="latest")
    args = parser.parse_args()

    if args.registry == "crates":
        publish_crates()
    else:
        publish_npm(args.manifest, args.tag)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
