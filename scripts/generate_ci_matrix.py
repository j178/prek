# /// script
# requires-python = ">=3.11"
# ///

from __future__ import annotations

import argparse
import json
from dataclasses import dataclass

LANGUAGE_FILTERS = {
    "bun": "test(bun::)",
    "dart": "test(dart::)",
    "deno": "test(deno::)",
    "docker": "test(docker::) or test(docker_image::)",
    "dotnet": "test(dotnet::)",
    "golang": "test(golang::)",
    "haskell": "test(haskell::)",
    "julia": "test(julia::)",
    "lua": "test(lua::)",
    "node": "test(node::)",
    "python": "test(python::)",
    "ruby": "test(ruby::)",
    "rust": "test(rust::)",
    "swift": "test(swift::)",
}

LANGUAGES = tuple(LANGUAGE_FILTERS)


@dataclass(frozen=True)
class Platform:
    os: str
    unsupported_languages: frozenset[str]

    def languages(self) -> list[str]:
        return [
            language
            for language in LANGUAGES
            if language not in self.unsupported_languages
        ]


PLATFORMS = (
    # Docker only runs on Ubuntu, Swift skips Windows, and Haskell skips macOS.
    Platform("ubuntu-latest", frozenset()),
    Platform("macos-latest", frozenset({"docker", "haskell"})),
    Platform("windows-latest", frozenset({"docker", "swift"})),
)


def chunks(items: list[str], size: int) -> list[list[str]]:
    return [items[index : index + size] for index in range(0, len(items), size)]


def language_filter(languages: list[str]) -> str:
    return " or ".join(LANGUAGE_FILTERS[language] for language in languages)


def ci_core_filter() -> str:
    # Exclude heavy language-specific integration tests from the main CI runs.
    excluded_languages = language_filter(list(LANGUAGES))

    # Keep this as a deny-list so new language test modules run in ci-core until
    # they are deliberately added to LANGUAGE_FILTERS and the language-test matrix.
    return (
        "not binary_id(prek::languages) or "
        f"(binary_id(prek::languages) and not ({excluded_languages}))"
    )


def generate_language_test_matrix(group_size: int) -> dict[str, list[dict[str, str | int]]]:
    include = []
    for platform in PLATFORMS:
        for group, languages in enumerate(chunks(platform.languages(), group_size), start=1):
            include.append(
                {
                    "os": platform.os,
                    "group": group,
                    "languages": " ".join(languages),
                    "filter": language_filter(languages),
                }
            )

    return {"include": include}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate GitHub Actions matrices for CI jobs.",
    )
    parser.add_argument(
        "--group-size",
        type=int,
        default=4,
        help="maximum number of languages per language-test job",
    )
    parser.add_argument(
        "--github-output",
        action="store_true",
        help="emit values in GitHub Actions output format",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    if args.group_size < 1:
        msg = "--group-size must be greater than zero"
        raise SystemExit(msg)

    language_test_matrix = json.dumps(
        generate_language_test_matrix(args.group_size),
        separators=(",", ":"),
    )
    core_filter = ci_core_filter()

    if args.github_output:
        print(f"language-test-matrix={language_test_matrix}")
        print(f"ci-core-filter={core_filter}")
    else:
        print(language_test_matrix)


if __name__ == "__main__":
    main()
