# Environment Variable Reference

`prek` supports the following environment variables:

## prek variables

### `PREK_HOME`

Override the prek data directory (caches, toolchains, hook envs).
If beginning with `~`, it is expanded to the user's home directory.
Defaults to `~/.cache/prek` on macOS and Linux, and `%LOCALAPPDATA%\prek` on Windows.

### `PREK_COLOR`

Control colored output: auto (default), always, or never.

### `PREK_QUIET`

Control quiet output mode.
Set to `1` for quiet mode (equivalent to `-q`, only shows failed hooks), or `2` for silent mode (equivalent to `-qq`, no output to stdout).

### `PREK_SKIP`

Comma-separated list of hook IDs to skip (e.g. black,ruff).
See [Skipping Projects or Hooks](../workspace.md#skipping-projects-or-hooks) for details.

### `PREK_ALLOW_NO_CONFIG`

Allow running without a configuration file (useful for ad-hoc runs).

### `PREK_NO_CONCURRENCY`

Disable parallelism for installs and runs.
If set, force concurrency to 1.

### `PREK_MAX_CONCURRENCY`

Set the maximum number of concurrent hooks (minimum 1).
Defaults to the number of CPU cores when unset.
Ignored when `PREK_NO_CONCURRENCY` is set.
If you encounter "Too many open files" errors, lowering this value or raising the file descriptor limit with `ulimit -n` can help.

### `PREK_NO_FAST_PATH`

Disable Rust-native built-in hooks; always use the original hook implementation.
See [Built-in Fast Hooks](../builtin.md) for details.

### `PREK_UV_SOURCE`

Control how uv (Python package installer) is installed.
Options:

- `github` (download from GitHub releases)
- `pypi` (install from PyPI)
- `tuna` (use Tsinghua University mirror)
- `aliyun` (use Alibaba Cloud mirror)
- `tencent` (use Tencent Cloud mirror)
- `pip` (install via pip)
- a custom PyPI mirror URL

If not set, prek automatically selects the best available source.

### `PREK_NATIVE_TLS`

Use the system trusted store instead of the bundled `webpki-roots` crate.

### `PREK_DOWNLOAD_CHECKSUM_POLICY`

Control checksum verification for managed toolchain downloads that use checksum sidecar files.
Options:

- `warn-missing` (default): verify downloads when a checksum is available; warn and continue when checksum metadata is missing
- `required`: require the checksum sidecar to be available and valid
- `disabled`: skip checksum fetching and verification

Checksum mismatches are hard errors whenever verification is enabled.

### `PREK_CONTAINER_RUNTIME`

Specify the container runtime to use for container-based hooks (e.g., `docker`, `docker_image`).
Options:

- `auto` (default, auto-detect available runtime)
- `docker`
- `podman`
- `container` (Apple's Container runtime on macOS, see [container](https://github.com/apple/container))

### `PREK_DOCKER_NO_INIT`

Disable passing the runtime's `--init` flag when running `docker` and `docker_image` hooks.
This is a compatibility escape hatch for container environments that cannot run the init helper.
Disabling `--init` can leave containers running after Ctrl-C if the container's PID 1 does not handle forwarded signals.

### `PREK_LOG_TRUNCATE_LIMIT`

Control the truncation limit for command lines shown in trace logs (`Executing ...`).
Defaults to `120` characters of arguments; set a larger value to reduce truncation.

### `PREK_RUBY_MIRROR`

Override the Ruby installer base URL used for downloaded Ruby toolchains (for example, when using mirrors or air-gapped CI environments).
Mirrors should provide release-compatible Ruby archive assets and a `SHA256SUMS` asset in the same release download location.
Only exact HTTPS GitHub repository mirrors (`https://github.com/owner/repo`, optionally with port `443`) receive `GITHUB_TOKEN`; other mirrors are used without GitHub authentication.
See [Ruby language support](../languages.md#ruby) for details.

### `PREK_RUST_PROFILE`

Override the `rustup` profile used when installing managed Rust toolchains (`minimal`, `default`, or `complete`). Defaults to `minimal`. Set to `default` to include `rustfmt` and `clippy`.
See [Rust language support](../languages.md#rust) for details.

## Compatibility fallbacks

### `PRE_COMMIT_ALLOW_NO_CONFIG`

Fallback for `PREK_ALLOW_NO_CONFIG`.

### `PRE_COMMIT_NO_CONCURRENCY`

Fallback for `PREK_NO_CONCURRENCY`.

### `SKIP`

Fallback for `PREK_SKIP`.
