# /// script
# requires-python = ">=3.11"
# ///

from __future__ import annotations

import argparse
import json
import shutil
import sys
import tarfile
import zipfile
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
NPM_ROOT = REPO_ROOT / "npm"


@dataclass(frozen=True)
class PlatformSpec:
    rust_target: str
    package_name: str
    archive_file: str
    binary_name: str
    os: list[str]
    cpu: list[str]
    libc: str | None = None
    arm_version_min: int | None = None
    arm_version_max: int | None = None

    def output_dir(self, base_dir: Path) -> Path:
        return base_dir.joinpath(*self.package_name.split("/"))

    def runtime_config(self) -> dict[str, object]:
        config: dict[str, object] = {
            "rustTarget": self.rust_target,
            "packageName": self.package_name,
            "binaryName": self.binary_name,
            "os": self.os,
            "cpu": self.cpu,
        }
        if self.libc is not None:
            config["libc"] = self.libc
        if self.arm_version_min is not None:
            config["armVersionMin"] = self.arm_version_min
        if self.arm_version_max is not None:
            config["armVersionMax"] = self.arm_version_max
        return config

    def package_json(self, version: str) -> dict[str, object]:
        package_json: dict[str, object] = {
            "name": self.package_name,
            "version": version,
            "description": f"Native {self.platform_label()} binary for prek.",
            "license": "MIT",
            "repository": {
                "type": "git",
                "url": "git+https://github.com/j178/prek.git",
            },
            "homepage": "https://prek.j178.dev/",
            "bugs": {
                "url": "https://github.com/j178/prek/issues",
            },
            "engines": {
                "node": ">=18",
            },
            "preferUnplugged": True,
            "os": self.os,
            "cpu": self.cpu,
            "files": [self.binary_name, "README.md", "LICENSE"],
        }
        if self.libc is not None:
            package_json["libc"] = [self.libc]
        return package_json

    def platform_label(self) -> str:
        parts = [*self.os, *self.cpu]
        if self.libc is not None:
            parts.append(self.libc)
        return " ".join(parts)


PLATFORMS = (
    PlatformSpec(
        rust_target="aarch64-apple-darwin",
        package_name="@j178/prek-darwin-arm64",
        archive_file="prek-aarch64-apple-darwin.tar.gz",
        binary_name="prek",
        os=["darwin"],
        cpu=["arm64"],
    ),
    PlatformSpec(
        rust_target="x86_64-apple-darwin",
        package_name="@j178/prek-darwin-x64",
        archive_file="prek-x86_64-apple-darwin.tar.gz",
        binary_name="prek",
        os=["darwin"],
        cpu=["x64"],
    ),
    PlatformSpec(
        rust_target="aarch64-unknown-linux-gnu",
        package_name="@j178/prek-linux-arm64-gnu",
        archive_file="prek-aarch64-unknown-linux-gnu.tar.gz",
        binary_name="prek",
        os=["linux"],
        cpu=["arm64"],
        libc="glibc",
    ),
    PlatformSpec(
        rust_target="aarch64-unknown-linux-musl",
        package_name="@j178/prek-linux-arm64-musl",
        archive_file="prek-aarch64-unknown-linux-musl.tar.gz",
        binary_name="prek",
        os=["linux"],
        cpu=["arm64"],
        libc="musl",
    ),
    PlatformSpec(
        rust_target="armv7-unknown-linux-gnueabihf",
        package_name="@j178/prek-linux-arm-gnueabihf",
        archive_file="prek-armv7-unknown-linux-gnueabihf.tar.gz",
        binary_name="prek",
        os=["linux"],
        cpu=["arm"],
        libc="glibc",
        arm_version_min=7,
    ),
    PlatformSpec(
        rust_target="arm-unknown-linux-musleabihf",
        package_name="@j178/prek-linux-arm-musleabihf",
        archive_file="prek-arm-unknown-linux-musleabihf.tar.gz",
        binary_name="prek",
        os=["linux"],
        cpu=["arm"],
        libc="musl",
    ),
    PlatformSpec(
        rust_target="armv7-unknown-linux-musleabihf",
        package_name="@j178/prek-linux-armv7-musleabihf",
        archive_file="prek-armv7-unknown-linux-musleabihf.tar.gz",
        binary_name="prek",
        os=["linux"],
        cpu=["arm"],
        libc="musl",
        arm_version_min=7,
    ),
    PlatformSpec(
        rust_target="i686-unknown-linux-gnu",
        package_name="@j178/prek-linux-ia32-gnu",
        archive_file="prek-i686-unknown-linux-gnu.tar.gz",
        binary_name="prek",
        os=["linux"],
        cpu=["ia32"],
        libc="glibc",
    ),
    PlatformSpec(
        rust_target="i686-unknown-linux-musl",
        package_name="@j178/prek-linux-ia32-musl",
        archive_file="prek-i686-unknown-linux-musl.tar.gz",
        binary_name="prek",
        os=["linux"],
        cpu=["ia32"],
        libc="musl",
    ),
    PlatformSpec(
        rust_target="riscv64gc-unknown-linux-gnu",
        package_name="@j178/prek-linux-riscv64-gnu",
        archive_file="prek-riscv64gc-unknown-linux-gnu.tar.gz",
        binary_name="prek",
        os=["linux"],
        cpu=["riscv64"],
        libc="glibc",
    ),
    PlatformSpec(
        rust_target="s390x-unknown-linux-gnu",
        package_name="@j178/prek-linux-s390x-gnu",
        archive_file="prek-s390x-unknown-linux-gnu.tar.gz",
        binary_name="prek",
        os=["linux"],
        cpu=["s390x"],
        libc="glibc",
    ),
    PlatformSpec(
        rust_target="x86_64-unknown-linux-gnu",
        package_name="@j178/prek-linux-x64-gnu",
        archive_file="prek-x86_64-unknown-linux-gnu.tar.gz",
        binary_name="prek",
        os=["linux"],
        cpu=["x64"],
        libc="glibc",
    ),
    PlatformSpec(
        rust_target="x86_64-unknown-linux-musl",
        package_name="@j178/prek-linux-x64-musl",
        archive_file="prek-x86_64-unknown-linux-musl.tar.gz",
        binary_name="prek",
        os=["linux"],
        cpu=["x64"],
        libc="musl",
    ),
    PlatformSpec(
        rust_target="aarch64-pc-windows-msvc",
        package_name="@j178/prek-win32-arm64-msvc",
        archive_file="prek-aarch64-pc-windows-msvc.zip",
        binary_name="prek.exe",
        os=["win32"],
        cpu=["arm64"],
    ),
    PlatformSpec(
        rust_target="i686-pc-windows-msvc",
        package_name="@j178/prek-win32-ia32-msvc",
        archive_file="prek-i686-pc-windows-msvc.zip",
        binary_name="prek.exe",
        os=["win32"],
        cpu=["ia32"],
    ),
    PlatformSpec(
        rust_target="x86_64-pc-windows-msvc",
        package_name="@j178/prek-win32-x64-msvc",
        archive_file="prek-x86_64-pc-windows-msvc.zip",
        binary_name="prek.exe",
        os=["win32"],
        cpu=["x64"],
    ),
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=("Build npm wrapper and platform packages from release archives."),
    )
    parser.add_argument(
        "--plan",
        type=Path,
        required=True,
        help="The cargo-dist plan JSON file.",
    )
    parser.add_argument(
        "--artifacts-dir",
        type=Path,
        default=REPO_ROOT / "npm-artifacts",
        help="Directory containing prek release archives.",
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=NPM_ROOT / ".output",
        help="Directory where npm package trees will be written.",
    )
    return parser.parse_args()


def read_plan_version(plan_path: Path) -> str:
    print(f"Reading version from dist plan: {plan_path}")
    with plan_path.open(encoding="utf-8") as file:
        plan = json.load(file)

    versions = sorted(
        {
            release["app_version"]
            for release in plan["releases"]
            if release["app_name"] == "prek"
        },
    )
    if len(versions) != 1:
        raise RuntimeError(
            f"Expected exactly one prek release version, got: {', '.join(versions)}",
        )
    return versions[0]


def create_wrapper_package(
    output_dir: Path,
    version: str,
    platforms: list[PlatformSpec],
) -> None:
    bin_dir = output_dir / "bin"
    bin_dir.mkdir(parents=True, exist_ok=True)
    shutil.copy2(NPM_ROOT / "bin" / "prek.js", bin_dir / "prek.js")
    (bin_dir / "prek.js").chmod(0o755)
    with (output_dir / "platforms.json").open("w", encoding="utf-8") as file:
        json.dump([platform.runtime_config() for platform in platforms], file, indent=2)
        file.write("\n")
    shutil.copy2(REPO_ROOT / "README.md", output_dir / "README.md")
    shutil.copy2(REPO_ROOT / "CHANGELOG.md", output_dir / "CHANGELOG.md")
    shutil.copy2(REPO_ROOT / "LICENSE", output_dir / "LICENSE")

    optional_dependencies = {platform.package_name: version for platform in platforms}

    package_json = {
        "name": "@j178/prek",
        "version": version,
        "description": (
            "A Git hook manager written in Rust, designed as a drop-in alternative "
            "to pre-commit."
        ),
        "license": "MIT",
        "repository": {
            "type": "git",
            "url": "git+https://github.com/j178/prek.git",
        },
        "homepage": "https://prek.j178.dev/",
        "bugs": {
            "url": "https://github.com/j178/prek/issues",
        },
        "bin": {
            "prek": "bin/prek.js",
        },
        "engines": {
            "node": ">=18",
        },
        "preferUnplugged": True,
        "files": ["bin", "platforms.json", "README.md", "CHANGELOG.md", "LICENSE"],
        "optionalDependencies": optional_dependencies,
    }
    with (output_dir / "package.json").open("w", encoding="utf-8") as file:
        json.dump(package_json, file, indent=2)
        file.write("\n")


def create_platform_package(
    artifacts_dir: Path,
    output_dir: Path,
    spec: PlatformSpec,
    version: str,
) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    shutil.copy2(REPO_ROOT / "LICENSE", output_dir / "LICENSE")
    (output_dir / "README.md").write_text(
        (
            f"{spec.package_name}\n\n"
            "Platform package for @j178/prek. Not meant to be installed directly.\n"
        ),
        encoding="utf-8",
    )

    archive_path = artifacts_dir / spec.archive_file
    binary_bytes = extract_binary(archive_path, spec.binary_name)
    binary_path = output_dir / spec.binary_name
    binary_path.write_bytes(binary_bytes)
    if binary_path.suffix != ".exe":
        binary_path.chmod(0o755)

    with (output_dir / "package.json").open("w", encoding="utf-8") as file:
        json.dump(spec.package_json(version), file, indent=2)
        file.write("\n")


def extract_binary(archive_path: Path, binary_name: str) -> bytes:
    if archive_path.suffixes[-2:] == [".tar", ".gz"]:
        return extract_from_tar_gz(archive_path, binary_name)
    if archive_path.suffix == ".zip":
        return extract_from_zip(archive_path, binary_name)
    raise RuntimeError(f"Unsupported archive format: {archive_path.name}")


def extract_from_tar_gz(archive_path: Path, binary_name: str) -> bytes:
    with tarfile.open(archive_path, mode="r:gz") as archive:
        for member in archive.getmembers():
            if Path(member.name).name != binary_name:
                continue
            extracted = archive.extractfile(member)
            if extracted is None:
                raise RuntimeError(
                    f"Failed to extract {member.name} from {archive_path.name}",
                )
            return extracted.read()
    raise RuntimeError(f"Could not find {binary_name} in {archive_path.name}")


def extract_from_zip(archive_path: Path, binary_name: str) -> bytes:
    with zipfile.ZipFile(archive_path) as archive:
        for member in archive.namelist():
            if Path(member).name == binary_name:
                return archive.read(member)
    raise RuntimeError(f"Could not find {binary_name} in {archive_path.name}")


def validate_artifacts_dir(artifacts_dir: Path, platforms: list[PlatformSpec]) -> None:
    if not artifacts_dir.is_dir():
        raise RuntimeError(
            f"Artifacts directory does not exist: {artifacts_dir}\n"
            "This script packages prebuilt release archives; it does not build them.\n"
            "Download the release artifacts into that directory or pass --artifacts-dir.",
        )

    missing = [
        platform.archive_file
        for platform in platforms
        if not (artifacts_dir / platform.archive_file).is_file()
    ]
    if missing:
        formatted_missing = "\n".join(f"  - {name}" for name in missing)
        raise RuntimeError(
            f"Missing binary archives in {artifacts_dir}:\n{formatted_missing}",
        )


def build_packages(version: str, artifacts_dir: Path, out_dir: Path) -> None:
    platforms = list(PLATFORMS)
    print(f"Building {len(platforms)} platform package(s) for prek {version}")

    print(f"Validating binary archives in {artifacts_dir}")
    validate_artifacts_dir(artifacts_dir, platforms)

    print(f"Writing npm packages to {out_dir}")
    shutil.rmtree(out_dir, ignore_errors=True)
    out_dir.mkdir(parents=True, exist_ok=True)

    wrapper_dir = out_dir / "@j178" / "prek"
    print(f"Building wrapper package: {wrapper_dir}")
    create_wrapper_package(wrapper_dir, version, platforms)

    platform_dirs: list[Path] = []
    for spec in platforms:
        package_dir = spec.output_dir(out_dir)
        print(f"Building platform package: {spec.package_name}")
        create_platform_package(artifacts_dir, package_dir, spec, version)
        platform_dirs.append(package_dir)

    manifest = {
        "version": version,
        "wrapper": str(wrapper_dir),
        "platforms": [str(path) for path in platform_dirs],
        "publishOrder": [str(path) for path in [*platform_dirs, wrapper_dir]],
    }
    with (out_dir / "manifest.json").open("w", encoding="utf-8") as file:
        json.dump(manifest, file, indent=2)
        file.write("\n")
    print(f"Wrote manifest: {out_dir / 'manifest.json'}")


def main() -> int:
    args = parse_args()
    version = read_plan_version(args.plan)
    build_packages(
        version,
        args.artifacts_dir.resolve(),
        args.out_dir.resolve(),
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        raise SystemExit(1)
