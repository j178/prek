# /// script
# requires-python = ">=3.14"
# ///

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import tomllib
from pathlib import Path
from typing import Any
from urllib.parse import urlencode
from urllib.request import urlopen


MACPORTS_REPO = "macports/macports-ports"
MACPORTS_BRANCH = "master"
MACPORTS_PORTFILE = "devel/prek/Portfile"

PULL_REQUEST_BODY_TEMPLATE = """#### Description

Update prek to {version}

###### Type(s)

- [ ] bugfix
- [ ] enhancement
- [ ] security fix

###### Tested on

macOS {macos_version} {macos_build} {arch}
Xcode {xcode_version} {xcode_build}

###### Verification
Have you

- [x] followed our [Commit Message Guidelines](https://trac.macports.org/wiki/CommitMessages)?
- [x] squashed and [minimized your commits](https://guide.macports.org/#project.github)?
- [x] checked that there are no other open [pull requests](https://github.com/macports/macports-ports/pulls) for the same change?
- [ ] referenced existing tickets on [Trac](https://trac.macports.org/wiki/Tickets) with full URL in commit message?
- [x] checked your Portfile with `port lint`?
- [x] tried existing tests with `sudo port test`?
- [x] tried a full install with `sudo port -vst install`?
- [x] tested basic functionality of all binary files?
- [x] checked that the Portfile most important variants have not been broken?
"""

PORTFILE_TEMPLATE = r"""# -*- coding: utf-8; mode: tcl; tab-width: 4; indent-tabs-mode: nil; c-basic-offset: 4 -*- vim:fenc=utf-8:ft=tcl:et:sw=4:ts=4:sts=4

PortSystem          1.0
PortGroup           cargo   1.0
PortGroup           github  1.0

github.setup        j178 prek {version} v
github.tarball_from archive
revision            0

description         A fast Git hook manager written in Rust, drop-in alternative to pre-commit.
long_description    {*}${description}

categories          devel
installs_libs       no
license             MIT
maintainers         {@j178 j178.dev:hi} openmaintainer
homepage            https://prek.j178.dev

checksums           ${distname}${extract.suffix} \
                    rmd160  {rmd160} \
                    sha256  {sha256} \
                    size    {size}

post-build {
    # Generate shell completions for supported shells
    set prek_bin ${worksrcpath}/target/[cargo.rust_platform]/release/${name}
    foreach shell {zsh bash fish} {
        system -W ${worksrcpath} "COMPLETE=${shell} ${prek_bin} > ${name}.${shell}"
    }
}

destroot {
    set bindir ${worksrcpath}/target/[cargo.rust_platform]/release
    xinstall -m 0755 ${bindir}/${name} ${destroot}${prefix}/bin/

    set zsh_comp_path ${destroot}${prefix}/share/zsh/site-functions
    xinstall -d ${zsh_comp_path}
    xinstall -m 0644 ${worksrcpath}/${name}.zsh ${zsh_comp_path}/_${name}

    set bash_comp_path ${destroot}${prefix}/share/bash-completion/completions
    xinstall -d ${bash_comp_path}
    xinstall -m 0644 ${worksrcpath}/${name}.bash ${bash_comp_path}/${name}

    set fish_comp_path ${destroot}${prefix}/share/fish/vendor_completions.d
    xinstall -d ${fish_comp_path}
    xinstall -m 0644 ${worksrcpath}/${name}.fish ${fish_comp_path}
}

build.args-append   -p prek

{cargo_crates}
"""


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


def current_tag(root: Path) -> str:
    return run(
        ["git", "-C", str(root), "describe", "--tags", "--abbrev=0"],
        capture=True,
    )


def download_distfile(version: str) -> Path:
    distfile = Path(f"/tmp/prek-v{version}.tar.gz")
    url = f"https://github.com/j178/prek/archive/v{version}.tar.gz"
    with urlopen(url, timeout=60) as response, distfile.open("wb") as output:
        shutil.copyfileobj(response, output)
    return distfile


def file_digest(algorithm: str, file_path: Path) -> str:
    with file_path.open("rb") as file:
        return hashlib.file_digest(file, algorithm).hexdigest()


def generate_cargo_crates(distfile: Path, version: str) -> str:
    cargo_lock_path = f"prek-{version}/Cargo.lock"
    with tarfile.open(distfile, "r:gz") as archive:
        try:
            cargo_lock = archive.extractfile(cargo_lock_path)
        except KeyError:
            raise RuntimeError(f"{cargo_lock_path} not found in {distfile}") from None
        if cargo_lock is None:
            raise RuntimeError(f"{cargo_lock_path} is not a file in {distfile}")
        with cargo_lock:
            packages = tomllib.load(cargo_lock)["package"]

    crates = [
        (package["name"], package["version"], package["checksum"])
        for package in packages
        if "checksum" in package
    ]
    if not crates:
        raise RuntimeError(f"No packages with checksums found in {cargo_lock_path}")

    lines = [
        f"    {name:<28}  {version:>8}  {checksum}"
        for name, version, checksum in crates
    ]
    return "cargo.crates \\\n" + " \\\n".join(lines)


def gh_api(
    endpoint: str,
    *,
    method: str = "GET",
    payload: dict[str, Any] | None = None,
    allow_not_found: bool = False,
) -> Any:
    cmd = ["gh", "api", "--method", method, endpoint]
    input_text = None
    if payload is not None:
        cmd.extend(["--input", "-"])
        input_text = json.dumps(payload)

    result = subprocess.run(
        cmd,
        check=False,
        text=True,
        input=input_text,
        capture_output=True,
    )
    if result.returncode != 0:
        if allow_not_found and "HTTP 404" in result.stderr:
            return None
        raise RuntimeError(result.stderr.strip())
    return json.loads(result.stdout)


def render_pull_request_body(version: str) -> str:
    xcode = run(["xcodebuild", "-version"], capture=True).splitlines()
    if len(xcode) != 2:
        raise RuntimeError(f"Unexpected xcodebuild output: {xcode}")

    return PULL_REQUEST_BODY_TEMPLATE.format(
        version=version,
        macos_version=run(["sw_vers", "-productVersion"], capture=True),
        macos_build=run(["sw_vers", "-buildVersion"], capture=True),
        arch=platform.machine(),
        xcode_version=xcode[0].removeprefix("Xcode "),
        xcode_build=xcode[1].removeprefix("Build version "),
    )


def create_macports_pull_request(version: str, portfile_text: str) -> str:
    login = gh_api("user")["login"]
    fork = f"{login}/macports-ports"
    branch = f"prek-{version}"
    body = render_pull_request_body(version)

    query = urlencode({"state": "open", "head": f"{login}:{branch}"})
    pull_requests = gh_api(f"repos/{MACPORTS_REPO}/pulls?{query}")
    if pull_requests:
        pull_request = gh_api(
            f"repos/{MACPORTS_REPO}/pulls/{pull_requests[0]['number']}",
            method="PATCH",
            payload={"body": body},
        )
        return pull_request["html_url"]

    fork_info = gh_api(f"repos/{fork}")
    if fork_info.get("parent", {}).get("full_name") != MACPORTS_REPO:
        raise RuntimeError(f"{fork} is not a fork of {MACPORTS_REPO}")

    branch_ref = gh_api(
        f"repos/{fork}/git/ref/heads/{branch}", allow_not_found=True
    )
    if branch_ref is not None:
        raise RuntimeError(f"Branch {fork}:{branch} already exists without an open PR")

    base_ref = gh_api(
        f"repos/{MACPORTS_REPO}/git/ref/heads/{MACPORTS_BRANCH}"
    )
    gh_api(
        f"repos/{fork}/git/refs",
        method="POST",
        payload={"ref": f"refs/heads/{branch}", "sha": base_ref["object"]["sha"]},
    )

    query = urlencode({"ref": branch})
    current_portfile = gh_api(
        f"repos/{fork}/contents/{MACPORTS_PORTFILE}?{query}"
    )
    title = f"prek: update to {version}"
    gh_api(
        f"repos/{fork}/contents/{MACPORTS_PORTFILE}",
        method="PUT",
        payload={
            "message": title,
            "content": base64.b64encode(portfile_text.encode()).decode(),
            "sha": current_portfile["sha"],
            "branch": branch,
        },
    )

    pull_request = gh_api(
        f"repos/{MACPORTS_REPO}/pulls",
        method="POST",
        payload={
            "title": title,
            "body": body,
            "head": f"{login}:{branch}",
            "base": MACPORTS_BRANCH,
            "draft": False,
            "maintainer_can_modify": True,
        },
    )
    return pull_request["html_url"]


def render_portfile(
    *, version: str, rmd160: str, sha256: str, size: int, cargo_crates: str
) -> str:
    return (
        PORTFILE_TEMPLATE.replace("{version}", version)
        .replace("{rmd160}", rmd160)
        .replace("{sha256}", sha256)
        .replace("{size}", str(size))
        .replace("{cargo_crates}", cargo_crates.rstrip())
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--submit", action="store_true", help="Create a pull request in MacPorts"
    )
    args = parser.parse_args()

    root = repo_root()
    default_portfile = Path("/tmp/prek-Portfile")
    portfile = Path(os.environ.get("PORTFILE", str(default_portfile)))

    version = current_tag(root).removeprefix("v")

    distfile = download_distfile(version)
    rmd160 = file_digest("ripemd160", distfile)
    sha256 = file_digest("sha256", distfile)
    size = distfile.stat().st_size

    cargo_crates = generate_cargo_crates(distfile, version)
    text = render_portfile(
        version=version,
        rmd160=rmd160,
        sha256=sha256,
        size=size,
        cargo_crates=cargo_crates,
    )

    portfile.write_text(text, encoding="utf-8")
    print(f"Generated {portfile} for version {version}")
    if args.submit:
        url = create_macports_pull_request(version, text)
        print(f"Pull request: {url}")
    else:
        print("To create a pull request, rerun with --submit")


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        raise SystemExit(1)
