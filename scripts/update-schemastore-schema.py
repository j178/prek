# /// script
# requires-python = ">=3.14"
# ///

from __future__ import annotations

import argparse
import base64
import json
import re
import subprocess
from pathlib import Path
from typing import Any
from urllib.parse import urlencode


SCHEMASTORE_REPO = "SchemaStore/schemastore"
SCHEMASTORE_BRANCH = "master"
SCHEMASTORE_SCHEMA = "src/schemas/json/prek.json"
# SchemaStore's Prettier config fixes these keys at the beginning or end while
# preserving the generated order of all other schema keys.
FIRST_SCHEMA_KEYS = ("$schema", "$id", "$comment", "$ref")
LAST_SCHEMA_KEYS = ("if", "then", "else")


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


def repo_root() -> Path:
    root = run(["git", "rev-parse", "--show-toplevel"], capture=True)
    return Path(root)


def read_version(cargo_toml: Path) -> str:
    content = cargo_toml.read_text(encoding="utf-8")
    match = re.search(r'^version\s*=\s*"([^"]+)"', content, flags=re.MULTILINE)
    if not match:
        raise RuntimeError(f"Failed to read version from {cargo_toml}")
    return match.group(1)


def sort_schema(value: Any) -> Any:
    if isinstance(value, dict):
        keys = list(value)
        ordered = [key for key in FIRST_SCHEMA_KEYS if key in value]
        ordered.extend(
            key for key in keys if key.startswith("$") and key not in ordered
        )
        ordered.extend(
            key
            for key in keys
            if not key.startswith("$") and key not in LAST_SCHEMA_KEYS
        )
        ordered.extend(key for key in LAST_SCHEMA_KEYS if key in value)
        return {key: sort_schema(value[key]) for key in ordered}
    if isinstance(value, list):
        return [sort_schema(item) for item in value]
    return value


def render_json(value: Any, *, indent: int = 0, column: int = 0) -> str:
    if isinstance(value, dict):
        if not value:
            return "{}"

        lines = []
        for key, item in value.items():
            key_text = json.dumps(key, ensure_ascii=False)
            prefix = " " * (indent + 2) + key_text + ": "
            lines.append(
                prefix
                + render_json(item, indent=indent + 2, column=len(prefix))
            )
        return "{\n" + ",\n".join(lines) + "\n" + " " * indent + "}"

    if isinstance(value, list):
        if not value:
            return "[]"

        if all(not isinstance(item, (dict, list)) for item in value):
            compact = "[" + ", ".join(render_json(item) for item in value) + "]"
            if column + len(compact) <= 80:
                return compact

        prefix = " " * (indent + 2)
        items = [
            prefix + render_json(item, indent=indent + 2, column=len(prefix))
            for item in value
        ]
        return "[\n" + ",\n".join(items) + "\n" + " " * indent + "]"

    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def format_schema(path: Path) -> str:
    schema = json.loads(path.read_text(encoding="utf-8"))
    return render_json(sort_schema(schema)) + "\n"


def decode_content(file: dict[str, Any]) -> str:
    return base64.b64decode(file["content"]).decode()


def create_schemastore_pull_request(version: str, schema: str) -> str | None:
    query = urlencode({"ref": SCHEMASTORE_BRANCH})
    upstream_schema = gh_api(
        f"repos/{SCHEMASTORE_REPO}/contents/{SCHEMASTORE_SCHEMA}?{query}"
    )
    if decode_content(upstream_schema) == schema:
        return None

    login = gh_api("user")["login"]
    fork = f"{login}/schemastore"
    branch = f"prek-{version}"
    title = f"Update prek schema to v{version}"

    fork_info = gh_api(f"repos/{fork}")
    if fork_info.get("parent", {}).get("full_name") != SCHEMASTORE_REPO:
        raise RuntimeError(f"{fork} is not a fork of {SCHEMASTORE_REPO}")

    pull_query = urlencode({"state": "open", "head": f"{login}:{branch}"})
    pull_requests = gh_api(f"repos/{SCHEMASTORE_REPO}/pulls?{pull_query}")
    if pull_requests:
        pull_request = pull_requests[0]
        branch_query = urlencode({"ref": branch})
        branch_schema = gh_api(
            f"repos/{fork}/contents/{SCHEMASTORE_SCHEMA}?{branch_query}"
        )
    else:
        branch_ref = gh_api(
            f"repos/{fork}/git/ref/heads/{branch}", allow_not_found=True
        )
        if branch_ref is not None:
            raise RuntimeError(
                f"Branch {fork}:{branch} already exists without an open PR"
            )

        base_ref = gh_api(
            f"repos/{SCHEMASTORE_REPO}/git/ref/heads/{SCHEMASTORE_BRANCH}"
        )
        gh_api(
            f"repos/{fork}/git/refs",
            method="POST",
            payload={
                "ref": f"refs/heads/{branch}",
                "sha": base_ref["object"]["sha"],
            },
        )
        branch_schema = upstream_schema

    if decode_content(branch_schema) != schema:
        gh_api(
            f"repos/{fork}/contents/{SCHEMASTORE_SCHEMA}",
            method="PUT",
            payload={
                "message": title,
                "content": base64.b64encode(schema.encode()).decode(),
                "sha": branch_schema["sha"],
                "branch": branch,
            },
        )

    if pull_requests:
        pull_request = gh_api(
            f"repos/{SCHEMASTORE_REPO}/pulls/{pull_request['number']}",
            method="PATCH",
            payload={"title": title, "body": title},
        )
    else:
        pull_request = gh_api(
            f"repos/{SCHEMASTORE_REPO}/pulls",
            method="POST",
            payload={
                "title": title,
                "body": title,
                "head": f"{login}:{branch}",
                "base": SCHEMASTORE_BRANCH,
                "draft": False,
                "maintainer_can_modify": True,
            },
        )
    return pull_request["html_url"]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--submit", action="store_true", help="Create a pull request in SchemaStore"
    )
    args = parser.parse_args()

    root = repo_root()
    version = read_version(root / "Cargo.toml")
    schema = format_schema(root / "prek.schema.json")

    if args.submit:
        url = create_schemastore_pull_request(version, schema)
        if url is None:
            print("SchemaStore schema is already up to date")
        else:
            print(f"Pull request: {url}")
    else:
        print(f"Prepared prek schema for v{version}")
        print("Rerun with --submit to create a SchemaStore pull request")


if __name__ == "__main__":
    main()
