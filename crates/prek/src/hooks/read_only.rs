/// Repositories and hooks whose normal operation does not modify tracked files.
/// Keep entries scoped to hook IDs: repositories can also ship fixers, and new hooks
/// need to be checked before they inherit a read-only default.
const READ_ONLY_HOOKS: &[(&str, phf::Map<&str, &[&str]>)] = &[
    (
        "github.com",
        phf::phf_map! {
            "abravalheri/validate-pyproject" => &["validate-pyproject"],
            "adrienverge/yamllint" => &["yamllint"],
            "asottile/dead" => &["dead"],
            "checkmake/checkmake" => &["checkmake", "checkmake-system"],
            "commitizen-tools/commitizen" => &["commitizen", "commitizen-branch"],
            "compilerla/conventional-pre-commit" => &["conventional-pre-commit"],
            "hadolint/hadolint" => &["hadolint", "hadolint-docker"],
            "henryiii/validate-pyproject-schema-store" => &["validate-pyproject"],
            "jendrikseipp/vulture" => &["vulture"],
            "koalaman/shellcheck-precommit" => &["shellcheck"],
            "Lucas-C/pre-commit-hooks" => &["forbid-crlf", "forbid-tabs"],
            "pre-commit/mirrors-csslint" => &["csslint"],
            "pre-commit/mirrors-jshint" => &["jshint"],
            "pre-commit/pre-commit-hooks" => &[
                "check-added-large-files",
                "check-ast",
                "check-builtin-literals",
                "check-case-conflict",
                "check-docstring-first",
                "check-executables-have-shebangs",
                "check-illegal-windows-names",
                "check-json",
                "check-merge-conflict",
                "check-shebang-scripts-are-executable",
                "check-symlinks",
                "check-toml",
                "check-vcs-permalinks",
                "check-xml",
                "check-yaml",
                "debug-statements",
                "destroyed-symlinks",
                "detect-aws-credentials",
                "detect-private-key",
                "forbid-new-submodules",
                "forbid-submodules",
                "name-tests-test",
                "no-commit-to-branch",
            ],
            "pre-commit/pygrep-hooks" => &[
                "python-check-blanket-noqa",
                "python-check-blanket-type-ignore",
                "python-check-mock-methods",
                "python-no-eval",
                "python-no-log-warn",
                "python-use-type-annotations",
                "rst-backticks",
                "rst-directive-colons",
                "rst-inline-touching-normal",
                "text-unicode-replacement-char",
            ],
            "PyCQA/doc8" => &["doc8"],
            "PyCQA/pydocstyle" => &["pydocstyle"],
            "python-jsonschema/check-jsonschema" => &[
                "check-azure-pipelines",
                "check-bamboo-spec",
                "check-bitbucket-pipelines",
                "check-buildkite",
                "check-changie",
                "check-circle-ci",
                "check-citation-file-format",
                "check-cloudbuild",
                "check-codecov",
                "check-compose-spec",
                "check-dependabot",
                "check-drone-ci",
                "check-github-actions",
                "check-github-discussion",
                "check-github-issue-config",
                "check-github-issue-forms",
                "check-github-workflows",
                "check-github-workflows-require-timeout",
                "check-gitlab-ci",
                "check-jsonschema",
                "check-meltano",
                "check-mergify",
                "check-metaschema",
                "check-readthedocs",
                "check-renovate",
                "check-snapcraft",
                "check-taskfile",
                "check-travis",
                "check-woodpecker-ci",
            ],
            "rstcheck/rstcheck" => &["rstcheck"],
            "seddonym/import-linter" => &["import-linter"],
            "shellcheck-py/shellcheck-py" => &["shellcheck"],
        },
    ),
    (
        "gitlab.com",
        phf::phf_map! {
            "bmares/check-json5" => &["check-json5"],
        },
    ),
];

pub(crate) fn is_known_read_only(url: &str, id: &str) -> bool {
    let Some((host, path)) = repo_parts(url) else {
        return false;
    };
    let Some((_, repos)) = READ_ONLY_HOOKS
        .iter()
        .find(|(known_host, _)| host.eq_ignore_ascii_case(known_host))
    else {
        return false;
    };

    repos.get(path).is_some_and(|hooks| hooks.contains(&id))
}

fn repo_parts(url: &str) -> Option<(&str, &str)> {
    let (authority, path) = if let Some((scheme, url)) = url.split_once("://") {
        match scheme {
            "http" | "https" | "ssh" | "git" => url.split_once('/')?,
            _ => return None,
        }
    } else {
        url.split_once(':')?
    };
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);

    // Preserve path case and explicit ports to avoid trusting a different repository.
    Some((host, path))
}
