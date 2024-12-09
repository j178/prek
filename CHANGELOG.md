# Changelog

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
