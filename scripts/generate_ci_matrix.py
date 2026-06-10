# /// script
# requires-python = ">=3.11"
# ///

from __future__ import annotations

import argparse
import json
from dataclasses import dataclass


@dataclass(frozen=True)
class LanguageTest:
    filter: str
    duration: int


# Approximate CI cost in seconds, based on
# https://github.com/j178/prek/actions/runs/27252262102?pr=2197.
# These are relative weights for balancing groups, not timeout guarantees.
LANGUAGE_TESTS = {
    "bun": LanguageTest("test(bun::)", 35),
    "coursier": LanguageTest("test(coursier::)", 35),
    "dart": LanguageTest("test(dart::)", 40),
    "deno": LanguageTest("test(deno::)", 40),
    "docker": LanguageTest("test(docker::) or test(docker_image::)", 30),
    "dotnet": LanguageTest("test(dotnet::)", 125),
    "golang": LanguageTest("test(golang::)", 90),
    "haskell": LanguageTest("test(haskell::)", 240),
    "julia": LanguageTest("test(julia::)", 110),
    "lua": LanguageTest("test(lua::)", 35),
    "node": LanguageTest("test(node::)", 35),
    "perl": LanguageTest("test(perl::)", 90),
    "python": LanguageTest("test(python::)", 60),
    "ruby": LanguageTest("test(ruby::)", 60),
    "rust": LanguageTest("test(rust::)", 125),
    "swift": LanguageTest("test(swift::)", 90),
}

LANGUAGES = tuple(LANGUAGE_TESTS)


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


def language_groups(languages: list[str], group_size: int) -> list[list[str]]:
    group_count = (len(languages) + group_size - 1) // group_size
    groups: list[list[str]] = [[] for _ in range(group_count)]
    group_durations = [0] * group_count
    language_order = {language: index for index, language in enumerate(LANGUAGES)}

    # Keep the minimum number of groups allowed by group_size, then use the
    # longest-processing-time heuristic to approximate bin packing: place each
    # language, starting with the most expensive one, into the currently lightest
    # group that still has capacity.
    for language in sorted(
        languages,
        key=lambda language: (-LANGUAGE_TESTS[language].duration, language_order[language]),
    ):
        group_index = min(
            (
                index
                for index, group in enumerate(groups)
                if len(group) < group_size
            ),
            key=lambda index: (group_durations[index], len(groups[index]), index),
        )
        groups[group_index].append(language)
        group_durations[group_index] += LANGUAGE_TESTS[language].duration

    return [
        sorted(group, key=lambda language: language_order[language])
        for group in groups
        if group
    ]


def language_filter(languages: list[str]) -> str:
    return " or ".join(LANGUAGE_TESTS[language].filter for language in languages)


def ci_core_filter() -> str:
    # Exclude heavy language-specific integration tests from the main CI runs.
    excluded_languages = language_filter(list(LANGUAGES))

    # Keep this as a deny-list so new language test modules run in ci-core until
    # they are deliberately added to LANGUAGE_TESTS and the language-test matrix.
    return (
        "not binary_id(prek::languages) or "
        f"(binary_id(prek::languages) and not ({excluded_languages}))"
    )


def generate_language_test_matrix(group_size: int) -> dict[str, list[dict[str, str | int]]]:
    include = []
    for platform in PLATFORMS:
        for group, languages in enumerate(
            language_groups(platform.languages(), group_size),
            start=1,
        ):
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
