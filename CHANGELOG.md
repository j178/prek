# Changelog

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
