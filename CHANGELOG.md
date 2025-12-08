# Changelog

## 0.2.20

Released on 2025-12-08.

### Highlights

In this release:

- Rust hooks are now fully supported with automatic toolchain management, including package discovery in virtual workspaces. Big thanks to @lmmx for driving this.
- Added a `prek cache size` subcommand so you can quickly see how much cache space prek is using. Thanks @MatthewMckee4!
- Nested workspaces are easier to reason about: set `orphan: true` on a project to isolate it from parents so its files are processed only once.

Want to show your project runs on prek? Add our README badge to your docs or repo homepage: [![prek](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/j178/prek/master/docs/assets/badge-v0.json)](https://github.com/j178/prek)

### Enhancements

- Support Rust language ([#989](https://github.com/j178/prek/pull/989))

  - Refactor Rust toolchain management ([#1198](https://github.com/j178/prek/pull/1198))
  - Add support for finding packages in virtual workspaces ([#1180](https://github.com/j178/prek/pull/1180))

- Add `prek cache size` command ([#1183](https://github.com/j178/prek/pull/1183))
- Support orphan projects ([#1129](https://github.com/j178/prek/pull/1129))
- Fallback to `manual` stage for hooks specified directly in command line ([#1185](https://github.com/j178/prek/pull/1185))
- Make go module cache read-writeable (thus deletable) ([#1164](https://github.com/j178/prek/pull/1164))
- Provide more information when validating configs and manifests ([#1182](https://github.com/j178/prek/pull/1182))
- Improve error message for invalid number of arguments to hook-impl ([#1196](https://github.com/j178/prek/pull/1196))

### Bug fixes

- Disable git terminal prompts ([#1193](https://github.com/j178/prek/pull/1193))
- Prevent `post-checkout` deadlock when cloning repos ([#1192](https://github.com/j178/prek/pull/1192))
- Prevent color output when redirecting stdout to a file ([#1159](https://github.com/j178/prek/pull/1159))

### Documentation

- Add MacPorts to installation methods ([#1157](https://github.com/j178/prek/pull/1157))
- Add a FAQ page explaining `prek install --install--hooks` ([#1162](https://github.com/j178/prek/pull/1162))

### Other changes

- Add `prek: enabled` repo badge ([#1171](https://github.com/j178/prek/pull/1171))
- Add favicon for docs website ([#1187](https://github.com/j178/prek/pull/1187))

### Contributors

- @MatthewMckee4
- @lmmx
- @j178
- @joshmarkovic
- @frazar
- @jmelahman
- @drainpixie

## 0.2.19

Released on 2025-11-26.

### Performance

- Simplify `fix_byte_order_marker` hook ([#1136](https://github.com/j178/prek/pull/1136))
- Simplify `trailing-whitespace` hook to improve performance ([#1135](https://github.com/j178/prek/pull/1135))

### Bug fixes

- Close stdin for hook subcommands ([#1155](https://github.com/j178/prek/pull/1155))
- Fix parsing Python interpreter info containing non-UTF8 chars ([#1141](https://github.com/j178/prek/pull/1141))

### Contributors

- @chilin0525
- @nblock
- @j178

## 0.2.18

Released on 2025-11-21.

### Highlights

In this release, prek adds a new special repo type `repo: builtin` that lets you use built‑in hooks.
It basically gives you another way to use the existing built‑in fast path for pre‑commit‑hooks, but without needing to point to an external repo.
Since prek doesn’t have to clone anything or set up a virtual environment, `repo: builtin` hooks work even in air‑gapped environments.

For more details, see: https://prek.j178.dev/builtin/

### Enhancements

- Add support `repo: builtin` ([#1118](https://github.com/j178/prek/pull/1118))
- Enable virtual terminal processing on Windows ([#1123](https://github.com/j178/prek/pull/1123))

### Bug fixes

- Do not recurse into submodules during workspace discovery ([#1121](https://github.com/j178/prek/pull/1121))
- Do not dim the hook output ([#1126](https://github.com/j178/prek/pull/1126))
- Further reduce max cli length for cmd.exe on Windows ([#1131](https://github.com/j178/prek/pull/1131))
- Revert "Disallow hook-level `minimum_prek_version` (#1101)" ([#1120](https://github.com/j178/prek/pull/1120))

### Other changes

- docs: refer airflow as Apache Airflow ([#1116](https://github.com/j178/prek/pull/1116))

### Contributors

- @j178
- @Lee-W

## 0.2.17

Released on 2025-11-18.

### Bug fixes

- Revert back to use `serde_yaml` again ([#1112](https://github.com/j178/prek/pull/1112))

### Contributors

- @j178

## 0.2.16

Released on 2025-11-18.

### Bug fixes

- Disallow hook-level `minimum_prek_version` ([#1101](https://github.com/j178/prek/pull/1101))
- Do not require a project in `prek init-template-dir` ([#1109](https://github.com/j178/prek/pull/1109))
- Make sure `uv pip install` uses the Python from virtualenv ([#1108](https://github.com/j178/prek/pull/1108))
- Restore using `serde_yaml` in `check-yaml` hook ([#1106](https://github.com/j178/prek/pull/1106))

### Contributors

- @j178

## 0.2.15

Released on 2025-11-17.

### Highlights

prek is now available on crates.io! You can build prek from source via `cargo install prek` or `cargo binstall prek`, for more details see [Installation](https://prek.j178.dev/installation/#build-from-source).

### Enhancements

- Clean up hook environments when install fails ([#1085](https://github.com/j178/prek/pull/1085))
- Prepare for publishing prek to crates.io ([#1088](https://github.com/j178/prek/pull/1088))
- Replace `serde-yaml` with `serde_saphyr` ([#1087](https://github.com/j178/prek/pull/1087))
- Warn unexpected keys in repo and hook level ([#1096](https://github.com/j178/prek/pull/1096))

### Bug fixes

- Fix `prek init-template-dir` fails in non-git repo ([#1093](https://github.com/j178/prek/pull/1093))

### Contributors

- @j178

## 0.2.14

Released on 2025-11-14.

### Enhancements

- Support `PREK_CONTAINER_RUNTIME=podman` to override container runtime ([#1033](https://github.com/j178/prek/pull/1033))
- Support rootless container runtime ([#1018](https://github.com/j178/prek/issues/1018))
- Support `language: unsupported` and `language: unsupported_script` introduced in pre-commit v4.4 ([#1073](https://github.com/j178/prek/pull/1073))
- Tweak to regex used for mountinfo ([#1037](https://github.com/j178/prek/pull/1037))

### Bug fixes

- Fix `--files` argument - files referencing other projects aren’t being filtered ([#1064](https://github.com/j178/prek/pull/1064))
- Unset `objectFormat` in `git init` ([#1048](https://github.com/j178/prek/pull/1048))

### Documentation

- Add scoop to installation ([#1067](https://github.com/j178/prek/pull/1067))
- Document workspace file visibility constraints ([#1071](https://github.com/j178/prek/pull/1071))
- Add `iceberg-python`, `msgspec` and `humanize` to "who is using prek" ([#1039](https://github.com/j178/prek/pull/1039), [#1042](https://github.com/j178/prek/pull/1042), [#1063](https://github.com/j178/prek/pull/1063))

### Other changes

- Add a hint to install when running inside a sub-project ([#1045](https://github.com/j178/prek/pull/1045))
- Add a hint to use `--refresh` when no configuration found ([#1046](https://github.com/j178/prek/pull/1046))
- Run uv pip install from the current directory ([#1069](https://github.com/j178/prek/pull/1069))

### Contributors

- @zzstoatzz
- @st1971
- @yihong0618
- @j178
- @copilot-swe-agent
- @idlsoft

## 0.2.13

Released on 2025-11-04.

### Enhancements

- Add Ruby support (no download support yet) ([#993](https://github.com/j178/prek/pull/993))
- Implement `check-executables-have-shebangs` as builtin-hook ([#924](https://github.com/j178/prek/pull/924))
- Improve container id detection ([#1031](https://github.com/j178/prek/pull/1031))

### Performance

- Optimize hot paths: reduce allocations ([#997](https://github.com/j178/prek/pull/997))
- Refactor `identify` using smallvec ([#982](https://github.com/j178/prek/pull/982))

### Bug fixes

- Fix YAML with nested merge keys ([#1020](https://github.com/j178/prek/pull/1020))
- Treat every file as executable on Windows to keep compatibility with pre-commit ([#980](https://github.com/j178/prek/pull/980))

### Documentation

- Document that .gitignore is respected by default during workspace discovery ([#983](https://github.com/j178/prek/pull/983))
- Update project stability status ([#1005](https://github.com/j178/prek/pull/1005))
- Add FastMCP to "who is using prek" ([#1034](https://github.com/j178/prek/pull/1034))
- Add attrs to "who is using prek" ([#981](https://github.com/j178/prek/pull/981))

### Contributors

- @my1e5
- @j178
- @zzstoatzz
- @lmmx
- @feliblo
- @yihong0618
- @st1971
- @is-alnilam

## 0.2.12

Released on 2025-10-27.

### Enhancements

- Add a warning for unimplemented hooks ([#976](https://github.com/j178/prek/pull/976))
- Allow using system trusted store by `PREK_NATIVE_TLS` ([#959](https://github.com/j178/prek/pull/959))

### Bug fixes

- Do not check for `script` subprocess status ([#964](https://github.com/j178/prek/pull/964))
- Fix compatibility with older luarocks ([#967](https://github.com/j178/prek/pull/967))
- Fix local relative path in `try-repo` ([#975](https://github.com/j178/prek/pull/975))

### Documentation

- Update language support status ([#970](https://github.com/j178/prek/pull/970))

### Contributors

- @yihong0618
- @st1971
- @j178

## 0.2.11

Released on 2025-10-24.

### Enhancements

- Support `language: lua` hooks ([#954](https://github.com/j178/prek/pull/954))
- Support `language_version: system` ([#949](https://github.com/j178/prek/pull/949))
- Implement `no-commit-to-branch` as builtin hook ([#930](https://github.com/j178/prek/pull/930))
- Improve styling for stashing error message ([#953](https://github.com/j178/prek/pull/953))
- Support nix-shell style shebang ([#929](https://github.com/j178/prek/pull/929))

### Documentation

- Add a page about "Quick start" ([#934](https://github.com/j178/prek/pull/934))
- Add kreuzberg to "who is using prek" ([#936](https://github.com/j178/prek/pull/936))
- Clarify minimum mise version required to use `mise use prek` ([#931](https://github.com/j178/prek/pull/931))

### Contributors

- @fllesser
- @j178

## 0.2.10

Released on 2025-10-18.

### Enhancements

- Add `--fail-fast` CLI flag to stop after first hook failure ([#908](https://github.com/j178/prek/pull/908))
- Add collision detection for hook env directories ([#914](https://github.com/j178/prek/pull/914))
- Error out if not projects found ([#913](https://github.com/j178/prek/pull/913))
- Implement `check-xml` as builtin hook ([#894](https://github.com/j178/prek/pull/894))
- Implement `check-merge-conflict` as builtin hook ([#885](https://github.com/j178/prek/pull/885))
- Use line-by-line reading in `check-merge-conflict` ([#910](https://github.com/j178/prek/pull/910))

### Bug fixes

- Fix pygrep hook env health check ([#921](https://github.com/j178/prek/pull/921))
- Group `pygrep` with `python` when installing pygrep hooks ([#920](https://github.com/j178/prek/pull/920))
- Ignore `.` prefixed directory when searching managed Python for pygrep ([#919](https://github.com/j178/prek/pull/919))

### Documentation

- Add contribution guide ([#912](https://github.com/j178/prek/pull/912))

### Other changes

### Contributors

- @AdityasWorks
- @j178
- @kenwoodjw
- @lmmx

## 0.2.9

Released on 2025-10-16.

### Enhancements

- Lazily check hook env health ([#897](https://github.com/j178/prek/pull/897))
- Implement `check-symlinks` as builtin hook ([#895](https://github.com/j178/prek/pull/895))
- Implement `detect-private-key` as builtin hook ([#893](https://github.com/j178/prek/pull/893))

### Bug fixes

- Download files to scratch directory to avoid cross-filesystem rename ([#889](https://github.com/j178/prek/pull/889))
- Fix golang hook install local dependencies ([#902](https://github.com/j178/prek/pull/902))
- Ignore the user-set `UV_MANAGED_PYTHON` ([#900](https://github.com/j178/prek/pull/900))

### Other changes

- Add package metadata for cargo-binstall ([#882](https://github.com/j178/prek/pull/882))

### Contributors

- @j178
- @lmmx

## 0.2.8

Released on 2025-10-14.

*This is a re-release of 0.2.6 that fixes an issue where publishing to npmjs.com failed.*

### Enhancements

- Publish prek to npmjs.com ([#819](https://github.com/j178/prek/pull/819))
- Support YAML merge keys in `.pre-commit-config.yaml` ([#871](https://github.com/j178/prek/pull/871))

### Bug fixes

- Use relative path with `--cd` in the generated hook script ([#868](https://github.com/j178/prek/pull/868))
- Fix autoupdate `rev` rendering for "float-like" version numbers ([#867](https://github.com/j178/prek/pull/867))

### Documentation

- Add Nix and Conda installation details ([#874](https://github.com/j178/prek/pull/874))

### Contributors

- @mondeja
- @j178
- @bbannier
- @yihong0618
- @colindean

## 0.2.5

Released on 2025-10-10.

### Enhancements

- Implement `prek try-repo` ([#797](https://github.com/j178/prek/pull/797))
- Add fallback mechanism for prek executable in git hooks ([#850](https://github.com/j178/prek/pull/850))
- Ignore config error if the directory is skipped ([#860](https://github.com/j178/prek/pull/860))

### Bug fixes

- Fix panic when parse config failed ([#859](https://github.com/j178/prek/pull/859))

### Other changes

- Add a Dockerfile ([#852](https://github.com/j178/prek/pull/852))

### Contributors

- @j178
- @luizvbo

## 0.2.4

Released on 2025-10-07.

### Enhancements

- Add support for `.prekignore` to ignore directories from project discovery ([#826](https://github.com/j178/prek/pull/826))
- Make `prek auto-update --jobs` default to 0 (which uses max available parallelism) ([#833](https://github.com/j178/prek/pull/833))
- Improve install message when installing for a subproject ([#847](https://github.com/j178/prek/pull/847))

### Bug fixes

- Convert extension to lowercase before checking file tags ([#839](https://github.com/j178/prek/pull/839))
- Support pass multiple files like `prek run --files a b c d` ([#828](https://github.com/j178/prek/pull/828))

### Documentation

- Add requests-cache to "Who is using prek" ([#824](https://github.com/j178/prek/pull/824))

### Contributors

- @SigureMo
- @j178

## 0.2.3

Released on 2025-09-29.

### Enhancements

- Add `--dry-run` to `prek auto-update` ([#806](https://github.com/j178/prek/pull/806))
- Add a global `--log-file` flag to specify the log file path ([#817](https://github.com/j178/prek/pull/817))
- Implement hook health check ([#798](https://github.com/j178/prek/pull/798))
- Show error message in quiet mode ([#807](https://github.com/j178/prek/pull/807))

### Bug fixes

- Write `fail` entry into output directly ([#811](https://github.com/j178/prek/pull/811))

### Documentation

- Update docs about uv in prek ([#810](https://github.com/j178/prek/pull/810))

### Other changes

- Add a security policy for reporting vulnerabilities ([#804](https://github.com/j178/prek/pull/804))

### Contributors

- @mondeja
- @j178

## 0.2.2

Released on 2025-09-26.

### Enhancements

- Add `prek cache dir`, move `prek gc` and `prek clean` under `prek cache` ([#795](https://github.com/j178/prek/pull/795))
- Add a hint when hooks failed in CI ([#800](https://github.com/j178/prek/pull/800))
- Add support for specifying `PREK_UV_SOURCE` ([#766](https://github.com/j178/prek/pull/766))
- Run docker container with `--init` ([#791](https://github.com/j178/prek/pull/791))
- Support `--allow-multiple-documents` for `check-yaml` ([#790](https://github.com/j178/prek/pull/790))

### Bug fixes

- Fix interpreter identification ([#801](https://github.com/j178/prek/pull/801))

### Documentation

- Add PaperQA2 to "Who is using prek" ([#793](https://github.com/j178/prek/pull/793))
- Clarify built-in hooks activation conditions and behavior ([#781](https://github.com/j178/prek/pull/781))
- Deduplicate docs between README and MkDocs site ([#792](https://github.com/j178/prek/pull/792))
- Mention `j178/prek-action` in docs ([#753](https://github.com/j178/prek/pull/753))

### Other Changes

- Bump `pre-commit-hooks` in sample-config to v6.0.0 ([#761](https://github.com/j178/prek/pull/761))
- Improve arg parsing for builtin hooks ([#789](https://github.com/j178/prek/pull/789))

### Contributors

- @mondeja
- @akx
- @bxb100
- @j178
- @onerandomusername

## 0.2.1

### Enhancements

- auto-update: prefer tags that are most similar to the current version ([#719](https://github.com/j178/prek/pull/719))

### Bug fixes

- Fix `git --no-pager diff` command syntax upon failures ([#746](https://github.com/j178/prek/pull/746))
- Clean working tree of current workspace only ([#747](https://github.com/j178/prek/pull/747))
- Use concurrent read and write in `git check-attr` ([#731](https://github.com/j178/prek/pull/731))

### Documentation

- Fix typo in language-version to language_version ([#727](https://github.com/j178/prek/pull/727))
- Update benchmarks ([#728](https://github.com/j178/prek/pull/728))

### Contributors

- @j178
- @matthiask
- @AdrianDC
- @onerandomusername

## 0.2.0

This is a huge milestone release that introduces **Workspace Mode** — first‑class monorepo support.

`prek` now allows you to manage multiple projects with their own `.pre-commit-config.yaml` within a single repository.
It auto‑discovers nested projects, runs hooks in project scope, and provides flexible selectors to target specific projects and hooks.
This makes `prek` a powerful tool for managing pre-commit hooks in complex repository structures.

For more details, see [Workspace Mode](https://prek.j178.dev/workspace/). If you encounter any issues, please report them at [Issues](https://github.com/j178/prek/issues).

**Note**: If you ran `prek install` in a repo before, you gonna need to run `prek install` again to replace the old git hook scripts for the workspace mode to work.

Special thanks to @potiuk for all the help and feedback in designing and testing this feature!

For detailed changes between 0.1.6 and 0.2.0, see [0.2.0-alpha.2](https://github.com/j178/prek/releases/v0.2.0-alpha.2), [0.2.0-alpha.3](https://github.com/j178/prek/releases/v0.2.0-alpha.3), [0.2.0-alpha.4](https://github.com/j178/prek/releases/v0.2.0-alpha.4), and [0.2.0-alpha.5](https://github.com/j178/prek/releases/v0.2.0-alpha.5).

### Enhancements

- Fix parsing of tag describe for prerelease versions ([#714](https://github.com/j178/prek/pull/714))
- Truncate log file each time ([#717](https://github.com/j178/prek/pull/717))

### Performance

- Enable more aggressive optimizations for release ([#724](https://github.com/j178/prek/pull/724))
- Speed up check_toml ([#713](https://github.com/j178/prek/pull/713))

### Bug fixes

- Fix hook-impl don't run hooks when specified allow missing config ([#716](https://github.com/j178/prek/pull/716))
- fix: support py38 for pygrep ([#723](https://github.com/j178/prek/pull/723))

### Other changes

- Fix installation on fish and with missing tags ([#721](https://github.com/j178/prek/pull/721))

### Contributors

- @onerandomusername
- @kushudai
- @j178

## 0.2.0a5

### Enhancements

- Add built in byte-order-marker fixer ([#700](https://github.com/j178/prek/pull/700))
- Use bigger buffer for fixing trailing whitespace ([#705](https://github.com/j178/prek/pull/705))

### Bug fixes

- Fix `trailing-whitespace` & `mixed-line-ending` write file path ([#708](https://github.com/j178/prek/pull/708))
- Fix file path handling for meta hooks in workspace mode ([#699](https://github.com/j178/prek/pull/699))

### Documentation

- Add docs about configuration ([#703](https://github.com/j178/prek/pull/703))
- Add docs about debugging ([#702](https://github.com/j178/prek/pull/702))
- Generate cli reference ([#707](https://github.com/j178/prek/pull/707))

### Contributors

- @kushudai
- @j178

## 0.2.0a4

### Enhancements

- Bring back `.pre-commit-config.yml` support ([#676](https://github.com/j178/prek/pull/676))
- Ignore config file from hidden directory ([#677](https://github.com/j178/prek/pull/677))
- Support selectors in `prek install/install-hooks/hook-impl` ([#683](https://github.com/j178/prek/pull/683))

### Bug fixes

- Do not set GOROOT for system install Go when running go hooks ([#694](https://github.com/j178/prek/pull/694))
- Fix `check_toml` and `check_yaml` in workspace mode ([#688](https://github.com/j178/prek/pull/688))

### Documentation

- Add docs about TODOs ([#679](https://github.com/j178/prek/pull/679))
- Add docs about builtin hooks ([#678](https://github.com/j178/prek/pull/678))

### Other changes

- docs(manifest): Correctly specify metadata for all packages ([#687](https://github.com/j178/prek/pull/687))
- refactor(cli): Clean up usage of clap ([#689](https://github.com/j178/prek/pull/689))

### Contributors

- @j178
- @epage
- @aravindan888

## 0.2.0a3

### Enhancements

- Add a warning to `hook-impl` when the script needs reinstall ([#647](https://github.com/j178/prek/pull/647))

### Documentation

- Add a notice to rerun `prek install` when upgrading to 0.2.0 ([#646](https://github.com/j178/prek/pull/646))

### Contributors

- @j178

## 0.2.0-alpha.2

*This is a re-release of [0.2.0-alpha.1](https://github.com/j178/prek/releases/tag/v0.2.0-alpha.1), fixed an issue that prereleases are not published to PyPI.*

This is a huge milestone release that introduces **Workspace Mode** — first‑class monorepo support.

`prek` now allows you to manage multiple projects with their own `.pre-commit-config.yaml` within a single repository. It auto‑discovers nested projects, runs hooks in project scope, and provides flexible selectors to target specific projects and hooks. This makes `prek` a powerful tool for managing pre-commit hooks in complex repository structures.

**Note**: If you ran `prek install` in a repo before, you gonna need to run `prek install` again to replace the old git hook scripts for the workspace mode to work.

For more details, see [Workspace Mode](https://prek.j178.dev/workspace/). If you encounter any issues, please report them at [Issues](https://github.com/j178/prek/issues).

Special thanks to @potiuk for all the help and feedback in designing and testing this feature!

### Enhancements

- Support multiple `.pre-commit-config.yaml` in a workspace (monorepo mode) ([#583](https://github.com/j178/prek/pull/583))
- Implement project and hook selector ([#623](https://github.com/j178/prek/pull/623))
- Add `prek run --cd <dir>` to change directory before running ([#581](https://github.com/j178/prek/pull/581))
- Support `prek list` in workspace mode ([#586](https://github.com/j178/prek/pull/586))
- Support `prek install|install-hooks|hook-impl|init-template-dir` in workspace mode ([#595](https://github.com/j178/prek/pull/595))
- Implement `auto-update` in workspace mode ([#605](https://github.com/j178/prek/pull/605))
- Implement selector completion in workspace mode ([#639](https://github.com/j178/prek/pull/639))
- Simplify `auto-update` implementation ([#608](https://github.com/j178/prek/pull/608))
- Add a `--dry-run` flag to `prek run` ([#622](https://github.com/j178/prek/pull/622))
- Cache workspace discovery result ([#636](https://github.com/j178/prek/pull/636))
- Fix local script hook entry path in workspace mode ([#603](https://github.com/j178/prek/pull/603))
- Fix `hook-impl` allow missing config ([#600](https://github.com/j178/prek/pull/600))
- Fix docker mount in workspace mode ([#638](https://github.com/j178/prek/pull/638))
- Show project line when project is not root ([#637](https://github.com/j178/prek/pull/637))

### Documentation

- Publish docs to `https://prek.j178.dev` ([#627](https://github.com/j178/prek/pull/627))
- Improve workspace docs about skips rule ([#615](https://github.com/j178/prek/pull/615))
- Add an full example and update docs ([#582](https://github.com/j178/prek/pull/582))

### Other changes

- Docs: `.pre-commit-config.yml` support has been removed ([#630](https://github.com/j178/prek/pull/630))
- Enable publishing prereleases ([#641](https://github.com/j178/prek/pull/641))

### Contributors

- [@luizvbo](https://github.com/luizvbo)
- [@j178](https://github.com/j178)
- [@hugovk](https://github.com/hugovk)

## 0.1.6

### Enhancements

- Improve hook install concurrency ([#611](https://github.com/j178/prek/pull/611))
- Parse JSON from slice ([#604](https://github.com/j178/prek/pull/604))

### Bug fixes

- Reuse hook env only for exactly same dependencies ([#609](https://github.com/j178/prek/pull/609))
- Workaround checkout file failure on Windows ([#616](https://github.com/j178/prek/pull/616))

## 0.1.5

### Enhancements

- Implement `pre-push` hook type ([#598](https://github.com/j178/prek/pull/598))
- Implement `pre-commit-hooks:check_yaml` as builtin hook ([#557](https://github.com/j178/prek/pull/557))
- Implement `pre-commit-hooks:check-toml` as builtin hook ([#564](https://github.com/j178/prek/pull/564))
- Add validation for file type tags ([#565](https://github.com/j178/prek/pull/565))
- Ignore NotFound error in extracting metadata log ([#597](https://github.com/j178/prek/pull/597))

### Documentation

- Update project status ([#578](https://github.com/j178/prek/pull/578))

### Other changes

- Bump tracing-subscriber to 0.3.20 ([#567](https://github.com/j178/prek/pull/567))
- Remove color from trace log ([#580](https://github.com/j178/prek/pull/580))

## 0.1.4

### Enhancements

- Improve docker image labels ([#551](https://github.com/j178/prek/pull/551))

### Performance

- Avoid unnecessary allocation in `run_by_batch` ([#549](https://github.com/j178/prek/pull/549))
- Cache current docker container mounts ([#552](https://github.com/j178/prek/pull/552))

### Bug fixes

- Fix `trailing-whitespace` cannot handle file contains invalid utf-8 data ([#544](https://github.com/j178/prek/pull/544))
- Fix trailing-whitespace eol trimming ([#546](https://github.com/j178/prek/pull/546))
- Fix trailing-whitespace markdown eol trimming ([#547](https://github.com/j178/prek/pull/547))

### Documentation

- Add authlib to `Who are using prek` ([#550](https://github.com/j178/prek/pull/550))

## 0.1.3

### Enhancements

- Support PEP 723 scripts for Python hooks ([#529](https://github.com/j178/prek/pull/529))

### Bug fixes

- Fix Python hook stderr are not captured ([#530](https://github.com/j178/prek/pull/530))

### Other changes

- Add an error context when reading manifest failed ([#527](https://github.com/j178/prek/pull/527))
- Add a renovate rule to bump bundled uv version ([#528](https://github.com/j178/prek/pull/528))
- Disable semantic commits for renovate PRs ([#538](https://github.com/j178/prek/pull/538))

## 0.1.2

### Enhancements

- Add check for missing hooks in new revision ([#521](https://github.com/j178/prek/pull/521))

### Bug fixes

- Fix `language: script` entry join issue ([#525](https://github.com/j178/prek/pull/525))

### Other changes

- Add OpenLineage to prek users ([#523](https://github.com/j178/prek/pull/523))

## 0.1.1

### Breaking changes

- Drop support `.yml` config file ([#493](https://github.com/j178/prek/pull/493))

### Enhancements

- Add moving rev warning ([#488](https://github.com/j178/prek/pull/488))
- Implement `prek auto-update` ([#511](https://github.com/j178/prek/pull/511))
- Support local path as a `repo` url ([#513](https://github.com/j178/prek/pull/513))

### Bug fixes

- Fix recursion limit when checking deeply nested json ([#507](https://github.com/j178/prek/pull/507))
- Fix rename tempfile across device ([#508](https://github.com/j178/prek/pull/508))
- Fix build on s390x ([#518](https://github.com/j178/prek/pull/518))

### Other changes

- docs: install prek with mise ([#510](https://github.com/j178/prek/pull/510))

## 0.0.29

### Enhancements

- Build wheels for more platforms ([#489](https://github.com/j178/prek/pull/489))

### Bug fixes

- Fix `git commit -a` does not pick up staged files correctly ([#487](https://github.com/j178/prek/pull/487))

## 0.0.28

### Bug fixes

- Fix `inde.lock file exists` error when running `git commit -p` or `git commit -a` ([#482](https://github.com/j178/prek/pull/482))
- Various fixes to `init-templdate-dir` and directory related bug ([#484](https://github.com/j178/prek/pull/484))

## 0.0.27

### Enhancements

- Clone repo temporarily into scratch directory ([#478](https://github.com/j178/prek/pull/478))
- Don’t show the progress bar if there’s no need for cloning or installing hooks ([#477](https://github.com/j178/prek/pull/477))
- Support `language_version: lts` for node ([#473](https://github.com/j178/prek/pull/473))

### Bug fixes

- Adjust `sample-config` file path before writing ([#474](https://github.com/j178/prek/pull/474))
- Resolve script shebang before running ([#475](https://github.com/j178/prek/pull/475))

## 0.0.26

### Enhancements

- Disable `prek self update` for package managers ([#468](https://github.com/j178/prek/pull/468))
- Download uv from github releases directly ([#464](https://github.com/j178/prek/pull/464))
- Find `uv` alongside the `prek` binary ([#466](https://github.com/j178/prek/pull/466))
- Run hooks with pty if color enabled ([#471](https://github.com/j178/prek/pull/471))
- Warn unexpected keys in config ([#463](https://github.com/j178/prek/pull/463))

### Bug fixes

- Canonicalize prek executable path ([#467](https://github.com/j178/prek/pull/467))

### Documentation

- Add "Who are using prek" to README ([#458](https://github.com/j178/prek/pull/458))

## 0.0.25

### Enhancements

- Add check for `minimum_prek_version` ([#437](https://github.com/j178/prefligit/pull/437))
- Make `--to-ref` default to HEAD if `--from-ref` is specified ([#426](https://github.com/j178/prefligit/pull/426))
- Support downloading uv from pypi and mirrors ([#449](https://github.com/j178/prefligit/pull/449))
- Write trace log to `$PREK_HOME/prek.log` ([#447](https://github.com/j178/prefligit/pull/447))
- Implement `mixed_line_ending` as builtin hook ([#444](https://github.com/j178/prefligit/pull/444))
- Support `--output-format=json` in `prek list` ([#446](https://github.com/j178/prefligit/pull/446))
- Add context message to install error ([#455](https://github.com/j178/prefligit/pull/455))
- Add warning for non-existent hook id ([#450](https://github.com/j178/prefligit/pull/450))

### Performance

- Refactor `fix_trailing_whitespace` ([#411](https://github.com/j178/prefligit/pull/411))

### Bug fixes

- Calculate more accurate max cli length ([#442](https://github.com/j178/prefligit/pull/442))
- Fix uv install on Windows ([#453](https://github.com/j178/prefligit/pull/453))
- Static link `liblzma` ([#445](https://github.com/j178/prefligit/pull/445))

## 0.0.24

### Enhancements

- Add dynamic completion of hook ids ([#380](https://github.com/j178/prek/pull/380))
- Implement `prek list` to list available hooks ([#424](https://github.com/j178/prek/pull/424))
- Implement `pygrep` language support ([#383](https://github.com/j178/prek/pull/383))
- Support `prek run` multiple hooks ([#423](https://github.com/j178/prek/pull/423))
- Implement `check_json` as builtin hook ([#416](https://github.com/j178/prek/pull/416))

### Performance

- Avoid reading whole file into memory in `fix_end_of_file` and make it consistent with `pre-commit-hooks` ([#399](https://github.com/j178/prek/pull/399))

### Bug fixes

- Do not set `GOROOT` and `GOPATH` for system found go ([#415](https://github.com/j178/prek/pull/415))

### Documentation

- Use `brew install j178/tap/prek` for now ([#420](https://github.com/j178/prek/pull/420))
- chore: logo rebranded, Update README.md ([#408](https://github.com/j178/prek/pull/408))

## 0.0.23

### Breaking changes

In this release, we've renamed the project to `prek` from `prefligit`. It's shorter so easier to type, and it avoids typosquatting with `preflight`.

This means that the command-line name is now `prek`, and the PyPI package is now listed as [`prek`](https://pypi.org/project/prek/).
And the Homebrew will be updated to `prek` as well.

And previously, the cache directory was `~/.cache/prefligit`, now it is `~/.cache/prek`.
You'd have to delete the old cache directory manually, or run `prefligit clean` to clean it up.

Then uninstall the old `prefligit` and install the new `prek` from scratch.

### Enhancements

- Relax uv version check range ([#396](https://github.com/j178/prefligit/pull/396))

### Bug fixes

- Fix `script` command path ([#398](https://github.com/j178/prefligit/pull/398))
- Fix meta hook `check_useless_excludes` ([#401](https://github.com/j178/prefligit/pull/401))

### Other changes

- Rename to `prek` from `prefligit` ([#402](https://github.com/j178/prefligit/pull/402))

## 0.0.22

### Enhancements

- Add value hint to `prefligit run` flags ([#373](https://github.com/j178/prefligit/pull/373))
- Check minimum supported version for uv found from system ([#352](https://github.com/j178/prefligit/pull/352))

### Bug fixes

- Fix `check_added_large_files` parameter name ([#389](https://github.com/j178/prefligit/pull/389))
- Fix `npm install` on Windows ([#374](https://github.com/j178/prefligit/pull/374))
- Fix docker mount options ([#377](https://github.com/j178/prefligit/pull/377))
- Fix identify tags for `Pipfile.lock` ([#391](https://github.com/j178/prefligit/pull/391))
- Fix identifying symlinks ([#378](https://github.com/j178/prefligit/pull/378))
- Set `GOROOT` when installing golang hook ([#381](https://github.com/j178/prefligit/pull/381))

### Other changes
- Add devcontainer config ([#379](https://github.com/j178/prefligit/pull/379))
- Bump rust toolchain to 1.89 ([#386](https://github.com/j178/prefligit/pull/386))

## 0.0.21

### Enhancements

- Add `--directory` to `prefligit run` ([#358](https://github.com/j178/prefligit/pull/358))
- Implement `tags_from_interpreter` ([#362](https://github.com/j178/prefligit/pull/362))
- Set GOBIN to `<hook-env>/bin`, set GOPATH to `$PREGLIGIT_HOME/cache/go` ([#369](https://github.com/j178/prefligit/pull/369))

### Performance

- Make Partitions iterator produce slice instead of Vec ([#361](https://github.com/j178/prefligit/pull/361))
- Use `rustc_hash` ([#359](https://github.com/j178/prefligit/pull/359))

### Bug fixes

- Add `node` to PATH when running `npm` ([#371](https://github.com/j178/prefligit/pull/371))
- Fix bug that default hook stage should be pre-commit ([#367](https://github.com/j178/prefligit/pull/367))
- Fix cache dir permission before clean ([#368](https://github.com/j178/prefligit/pull/368))

### Other changes

- Move `Project` into `workspace` module ([#364](https://github.com/j178/prefligit/pull/364))

## 0.0.20

### Enhancements

- Support golang hooks and golang toolchain management ([#355](https://github.com/j178/prefligit/pull/355))
- Add `--last-commit` flag to `prefligit run` ([#351](https://github.com/j178/prefligit/pull/351))

### Bug fixes

- Fix bug that directories are ignored ([#350](https://github.com/j178/prefligit/pull/350))
- Use `git ls-remote` to fetch go releases ([#356](https://github.com/j178/prefligit/pull/356))

### Documentation

- Add migration section to README ([#354](https://github.com/j178/prefligit/pull/354))

## 0.0.19

### Enhancements

- Improve node support ([#346](https://github.com/j178/prefligit/pull/346))
- Manage uv cache dir ([#345](https://github.com/j178/prefligit/pull/345))

### Bug fixes

- Add `--install-links` to `npm install` ([#347](https://github.com/j178/prefligit/pull/347))
- Fix large file check to use staged_get instead of intent_add ([#332](https://github.com/j178/prefligit/pull/332))

## 0.0.18

### Enhancements

- Impl `FromStr` for language request ([#338](https://github.com/j178/prefligit/pull/338))

### Performance

- Use DFS to find connected components in hook dependencies ([#341](https://github.com/j178/prefligit/pull/341))
- Use more `Arc<T>` over `Box<T>` ([#333](https://github.com/j178/prefligit/pull/333))

### Bug fixes

- Fix node path match, add tests ([#339](https://github.com/j178/prefligit/pull/339))
- Skipped hook name should be taken into account for columns ([#335](https://github.com/j178/prefligit/pull/335))

### Documentation

- Add benchmarks ([#342](https://github.com/j178/prefligit/pull/342))
- Update docs ([#337](https://github.com/j178/prefligit/pull/337))

## 0.0.17

### Enhancements

- Add `sample-config --file` to write sample config to file ([#313](https://github.com/j178/prefligit/pull/313))
- Cache computed `dependencies` on hook ([#319](https://github.com/j178/prefligit/pull/319))
- Cache the found path to uv ([#323](https://github.com/j178/prefligit/pull/323))
- Improve `sample-config` writing file ([#314](https://github.com/j178/prefligit/pull/314))
- Reimplement find matching env logic ([#327](https://github.com/j178/prefligit/pull/327))

### Bug fixes

- Fix issue that `entry` of `pygrep` is not shell commands ([#316](https://github.com/j178/prefligit/pull/316))
- Support `python311` as a valid language version ([#321](https://github.com/j178/prefligit/pull/321))

### Other changes

- Bump cargo-dist to 0.29.0 ([#322](https://github.com/j178/prefligit/pull/322))
- Update DIFF.md ([#318](https://github.com/j178/prefligit/pull/318))

## 0.0.16

### Enhancements

- Improve error message for hook ([#308](https://github.com/j178/prefligit/pull/308))
- Improve error message for hook installation and run ([#310](https://github.com/j178/prefligit/pull/310))
- Improve hook invalid error message ([#307](https://github.com/j178/prefligit/pull/307))
- Parse `entry` when constructing hook ([#306](https://github.com/j178/prefligit/pull/306))
- Rename `autoupdate` to `auto-update`, `init-templatedir` to `init-template-dir` ([#302](https://github.com/j178/prefligit/pull/302))

### Bug fixes

- Fix `end-of-file-fixer` replaces `\r\n` with `\n` ([#311](https://github.com/j178/prefligit/pull/311))

## 0.0.15

In this release, `language: node` hooks are fully supported now (finally)!.
Give it a try and let us know if you run into any issues!

### Enhancements

- Support `nodejs` language hook ([#298](https://github.com/j178/prefligit/pull/298))
- Show unimplemented message earlier ([#296](https://github.com/j178/prefligit/pull/296))
- Simplify npm installing dependencies ([#299](https://github.com/j178/prefligit/pull/299))

### Documentation

- Update readme ([#300](https://github.com/j178/prefligit/pull/300))

## 0.0.14

### Enhancements

- Show unimplemented status instead of panic ([#290](https://github.com/j178/prefligit/pull/290))
- Try default uv managed python first, fallback to download ([#291](https://github.com/j178/prefligit/pull/291))

### Other changes

- Update Rust crate fancy-regex to 0.16.0 ([#286](https://github.com/j178/prefligit/pull/286))
- Update Rust crate indicatif to 0.18.0 ([#287](https://github.com/j178/prefligit/pull/287))
- Update Rust crate pprof to 0.15.0 ([#288](https://github.com/j178/prefligit/pull/288))
- Update Rust crate serde_json to v1.0.142 ([#285](https://github.com/j178/prefligit/pull/285))
- Update astral-sh/setup-uv action to v6 ([#289](https://github.com/j178/prefligit/pull/289))

## 0.0.13

### Enhancements

- Add `PREFLIGIT_NO_FAST_PATH` to disable Rust fast path ([#272](https://github.com/j178/prefligit/pull/272))
- Improve subprocess error message ([#276](https://github.com/j178/prefligit/pull/276))
- Remove `LanguagePreference` and improve language check ([#277](https://github.com/j178/prefligit/pull/277))
- Support downloading requested Python version automatically ([#281](https://github.com/j178/prefligit/pull/281))
- Implement language specific version parsing ([#273](https://github.com/j178/prefligit/pull/273))

### Bug fixes

- Fix python version matching ([#275](https://github.com/j178/prefligit/pull/275))
- Show progress bar in verbose mode ([#278](https://github.com/j178/prefligit/pull/278))

## 0.0.12

### Bug fixes

- Ignore `config not staged` error for config outside the repo ([#270](https://github.com/j178/prefligit/pull/270))

### Other changes

- Add test fixture files ([#266](https://github.com/j178/prefligit/pull/266))
- Use `sync_all` over `flush` ([#269](https://github.com/j178/prefligit/pull/269))

## 0.0.11

### Enhancements

- Support reading `.pre-commit-config.yml` as well ([#213](https://github.com/j178/prefligit/pull/213))
- Refactor language version resolution and hook install dir ([#221](https://github.com/j178/prefligit/pull/221))
- Implement `prefligit install-hooks` command ([#258](https://github.com/j178/prefligit/pull/258))
- Implement `pre-commit-hooks:end-of-file-fixer` hook ([#255](https://github.com/j178/prefligit/pull/255))
- Implement `pre-commit-hooks:check_added_large_files` hook ([#219](https://github.com/j178/prefligit/pull/219))
- Implement `script` language hooks ([#252](https://github.com/j178/prefligit/pull/252))
- Implement node.js installer ([#152](https://github.com/j178/prefligit/pull/152))
- Use `-v` to show only verbose message, `-vv` show debug log, `-vvv` show trace log ([#211](https://github.com/j178/prefligit/pull/211))
- Write `.prefligit-repo.json` inside cloned repo ([#225](https://github.com/j178/prefligit/pull/225))
- Add language name to 'not yet implemented' messages ([#251](https://github.com/j178/prefligit/pull/251))

### Bug fixes

- Do not install if no additional dependencies for local python hook ([#195](https://github.com/j178/prefligit/pull/195))
- Ensure flushing log file ([#261](https://github.com/j178/prefligit/pull/261))
- Fix zip deflate ([#194](https://github.com/j178/prefligit/pull/194))

### Other changes

- Bump to Rust 1.88 and `cargo update` ([#254](https://github.com/j178/prefligit/pull/254))
- Upgrade to Rust 2024 edition ([#196](https://github.com/j178/prefligit/pull/196))
- Bump uv version ([#260](https://github.com/j178/prefligit/pull/260))
- Simplify archive extraction implementation ([#193](https://github.com/j178/prefligit/pull/193))
- Use `astral-sh/rs-async-zip` ([#259](https://github.com/j178/prefligit/pull/259))
- Use `ubuntu-latest` for release action ([#216](https://github.com/j178/prefligit/pull/216))
- Use async closure ([#200](https://github.com/j178/prefligit/pull/200))

## 0.0.10

### Breaking changes

**Warning**: This release changed the store layout, it's recommended to delete the old store and install from scratch.

To delete the old store, run:

```sh
rm -rf ~/.cache/prefligit
```

### Enhancements

- Restructure store folders layout ([#181](https://github.com/j178/prefligit/pull/181))
- Fallback some env vars to to pre-commit ([#175](https://github.com/j178/prefligit/pull/175))
- Save patches to `$PREFLIGIT_HOME/patches` ([#182](https://github.com/j178/prefligit/pull/182))

### Bug fixes

- Fix removing git env vars ([#176](https://github.com/j178/prefligit/pull/176))
- Fix typo in Cargo.toml ([#160](https://github.com/j178/prefligit/pull/160))

### Other changes

- Do not publish to crates.io ([#191](https://github.com/j178/prefligit/pull/191))
- Bump cargo-dist to v0.28.0 ([#170](https://github.com/j178/prefligit/pull/170))
- Bump uv version to 0.6.0 ([#184](https://github.com/j178/prefligit/pull/184))
- Configure Renovate ([#168](https://github.com/j178/prefligit/pull/168))
- Format sample config output ([#172](https://github.com/j178/prefligit/pull/172))
- Make env vars a shareable crate ([#171](https://github.com/j178/prefligit/pull/171))
- Reduce String alloc ([#166](https://github.com/j178/prefligit/pull/166))
- Skip common git flags in command trace log ([#162](https://github.com/j178/prefligit/pull/162))
- Update Rust crate clap to v4.5.29 ([#173](https://github.com/j178/prefligit/pull/173))
- Update Rust crate which to v7.0.2 ([#163](https://github.com/j178/prefligit/pull/163))
- Update astral-sh/setup-uv action to v5 ([#164](https://github.com/j178/prefligit/pull/164))
- Upgrade Rust to 1.84 and upgrade dependencies ([#161](https://github.com/j178/prefligit/pull/161))

## 0.0.9

Due to a mistake in the release process, this release is skipped.

## 0.0.8

### Enhancements

- Move home dir to `~/.cache/prefligit` ([#154](https://github.com/j178/prefligit/pull/154))
- Implement trailing-whitespace in Rust ([#137](https://github.com/j178/prefligit/pull/137))
- Limit hook install concurrency ([#145](https://github.com/j178/prefligit/pull/145))
- Simplify language default version implementation ([#150](https://github.com/j178/prefligit/pull/150))
- Support install uv from pypi ([#149](https://github.com/j178/prefligit/pull/149))
- Add executing command to error message ([#141](https://github.com/j178/prefligit/pull/141))

### Bug fixes

- Use hook `args` in fast path ([#139](https://github.com/j178/prefligit/pull/139))

### Other changes

- Remove hook install_key ([#153](https://github.com/j178/prefligit/pull/153))
- Remove pyvenv.cfg patch ([#156](https://github.com/j178/prefligit/pull/156))
- Try to use D drive on Windows CI ([#157](https://github.com/j178/prefligit/pull/157))
- Tweak trailing-whitespace-fixer ([#140](https://github.com/j178/prefligit/pull/140))
- Upgrade dist to v0.27.0 ([#158](https://github.com/j178/prefligit/pull/158))
- Uv install python into tools path ([#151](https://github.com/j178/prefligit/pull/151))

## 0.0.7

### Enhancements

- Add progress bar for hook init and install ([#122](https://github.com/j178/prefligit/pull/122))
- Add color to command help ([#131](https://github.com/j178/prefligit/pull/131))
- Add commit info to version display ([#130](https://github.com/j178/prefligit/pull/130))
- Support meta hooks reading ([#134](https://github.com/j178/prefligit/pull/134))
- Implement meta hooks ([#135](https://github.com/j178/prefligit/pull/135))

### Bug fixes

- Fix same repo clone multiple times ([#125](https://github.com/j178/prefligit/pull/125))
- Fix logging level after renaming ([#119](https://github.com/j178/prefligit/pull/119))
- Fix version tag distance ([#132](https://github.com/j178/prefligit/pull/132))

### Other changes

- Disable uv cache on Windows ([#127](https://github.com/j178/prefligit/pull/127))
- Impl Eq and Hash for ConfigRemoteRepo ([#126](https://github.com/j178/prefligit/pull/126))
- Make `pass_env_vars` runs on Windows ([#133](https://github.com/j178/prefligit/pull/133))
- Run cargo update ([#129](https://github.com/j178/prefligit/pull/129))
- Update Readme ([#128](https://github.com/j178/prefligit/pull/128))

## 0.0.6

### Breaking changes

In this release, we’ve renamed the project to `prefligit` (a deliberate misspelling of preflight) to prevent confusion with the existing pre-commit tool. For further information, refer to issue #73.

- The command-line name is now `prefligit`. We suggest uninstalling any previous version of `pre-commit-rs` and installing `prefligit` from scratch.
- The PyPI package is now listed as [`prefligit`](https://pypi.org/project/prefligit/).
- The Cargo package is also now [`prefligit`](https://crates.io/crates/prefligit).
- The Homebrew formula has been updated to `prefligit`.

### Enhancements

- Support `docker_image` language ([#113](https://github.com/j178/pre-commit-rs/pull/113))
- Support `init-templatedir` subcommand ([#101](https://github.com/j178/pre-commit-rs/pull/101))
- Implement get filenames from merge conflicts ([#103](https://github.com/j178/pre-commit-rs/pull/103))

### Bug fixes

- Fix `prefligit install --hook-type` name ([#102](https://github.com/j178/pre-commit-rs/pull/102))

### Other changes

- Apply color option to log ([#100](https://github.com/j178/pre-commit-rs/pull/100))
- Improve tests ([#106](https://github.com/j178/pre-commit-rs/pull/106))
- Remove intermedia Language enum ([#107](https://github.com/j178/pre-commit-rs/pull/107))
- Run `cargo clippy` in the dev drive workspace ([#115](https://github.com/j178/pre-commit-rs/pull/115))

## 0.0.5

### Enhancements

v0.0.4 release process was broken, so this release is a actually a re-release of v0.0.4.

- Improve subprocess trace and error output ([#92](https://github.com/j178/pre-commit-rs/pull/92))
- Stash working tree before running hooks ([#96](https://github.com/j178/pre-commit-rs/pull/96))
- Add color to command trace ([#94](https://github.com/j178/pre-commit-rs/pull/94))
- Improve hook output display ([#79](https://github.com/j178/pre-commit-rs/pull/79))
- Improve uv installation ([#78](https://github.com/j178/pre-commit-rs/pull/78))
- Support docker language ([#67](https://github.com/j178/pre-commit-rs/pull/67))

## 0.0.4

### Enhancements

- Improve subprocess trace and error output ([#92](https://github.com/j178/pre-commit-rs/pull/92))
- Stash working tree before running hooks ([#96](https://github.com/j178/pre-commit-rs/pull/96))
- Add color to command trace ([#94](https://github.com/j178/pre-commit-rs/pull/94))
- Improve hook output display ([#79](https://github.com/j178/pre-commit-rs/pull/79))
- Improve uv installation ([#78](https://github.com/j178/pre-commit-rs/pull/78))
- Support docker language ([#67](https://github.com/j178/pre-commit-rs/pull/67))

## 0.0.3

### Bug fixes

- Check uv installed after acquired lock ([#72](https://github.com/j178/pre-commit-rs/pull/72))

### Other changes

- Add copyright of the original pre-commit to LICENSE ([#74](https://github.com/j178/pre-commit-rs/pull/74))
- Add profiler ([#71](https://github.com/j178/pre-commit-rs/pull/71))
- Publish to PyPI ([#70](https://github.com/j178/pre-commit-rs/pull/70))
- Publish to crates.io ([#75](https://github.com/j178/pre-commit-rs/pull/75))
- Rename pypi package to `pre-commit-rusty` ([#76](https://github.com/j178/pre-commit-rs/pull/76))

## 0.0.2

### Enhancements

- Add `pre-commit self update` ([#68](https://github.com/j178/pre-commit-rs/pull/68))
- Auto install uv ([#66](https://github.com/j178/pre-commit-rs/pull/66))
- Generate shell completion ([#20](https://github.com/j178/pre-commit-rs/pull/20))
- Implement `pre-commit clean` ([#24](https://github.com/j178/pre-commit-rs/pull/24))
- Implement `pre-commit install` ([#28](https://github.com/j178/pre-commit-rs/pull/28))
- Implement `pre-commit sample-config` ([#37](https://github.com/j178/pre-commit-rs/pull/37))
- Implement `pre-commit uninstall` ([#36](https://github.com/j178/pre-commit-rs/pull/36))
- Implement `pre-commit validate-config` ([#25](https://github.com/j178/pre-commit-rs/pull/25))
- Implement `pre-commit validate-manifest` ([#26](https://github.com/j178/pre-commit-rs/pull/26))
- Implement basic `pre-commit hook-impl` ([#63](https://github.com/j178/pre-commit-rs/pull/63))
- Partition filenames and delegate to multiple subprocesses ([#7](https://github.com/j178/pre-commit-rs/pull/7))
- Refactor xargs ([#8](https://github.com/j178/pre-commit-rs/pull/8))
- Skip empty config argument ([#64](https://github.com/j178/pre-commit-rs/pull/64))
- Use `fancy-regex` ([#62](https://github.com/j178/pre-commit-rs/pull/62))
- feat: add fail language support ([#60](https://github.com/j178/pre-commit-rs/pull/60))

### Bug Fixes

- Fix stage operate_on_files ([#65](https://github.com/j178/pre-commit-rs/pull/65))
