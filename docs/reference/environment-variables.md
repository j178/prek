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

Comma-separated list of selectors to skip. A selector can be a hook ID, a
project path ending in `/`, or a project-qualified hook such as
`frontend:eslint`. For example, `PREK_SKIP=ruff,frontend/` skips every `ruff`
hook and the `frontend` project.
See [Skipping Projects or Hooks](../workspace.md#skipping-projects-or-hooks) for details.

### `PREK_ALLOW_NO_CONFIG`

Allow running without a configuration file (useful for ad-hoc runs).

### `PREK_NO_CONCURRENCY`

Disable hook and batch parallelism during `prek run`.
If set, force `PREK_CONCURRENT_HOOKS` and `PREK_CONCURRENT_BATCHES` to 1.

### `PREK_CONCURRENT_HOOKS`

Set the maximum number of hooks that can run at once during `prek run` (minimum 1).
Defaults to the number of CPU cores when unset.
Ignored when `PREK_NO_CONCURRENCY` is set.

### `PREK_CONCURRENT_BATCHES`

Set the maximum number of batches that each hook can run at once during `prek run` (minimum 1).
A batch is one hook command invocation over a subset of the matched filenames.
Defaults to the number of CPU cores when unset.
Ignored when `PREK_NO_CONCURRENCY` is set.

### `PREK_NO_FAST_PATH`

Disable Rust-native built-in hooks; always use the original hook implementation.
See [Built-in Fast Hooks](../builtin.md) for details.

### `PREK_UV_SOURCE`

Choose one source for installing uv, the Python package installer.
Options:

- `astral` (download from Astral's CDN)
- `github` (download from GitHub releases)
- `pypi` (install from PyPI)
- `tuna` (use Tsinghua University mirror)
- `aliyun` (use Alibaba Cloud mirror)
- `tencent` (use Tencent Cloud mirror)
- `pip` (install via pip)
- a custom PyPI mirror URL

If not set, prek tries Astral's CDN, PyPI and its configured mirrors, then `pip`
until one succeeds. The `github` source is used only when selected explicitly.

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

### `PREK_RUBY_MIRROR`

Override the Ruby installer base URL used for downloaded Ruby toolchains (for example, when using mirrors or air-gapped CI environments).
Mirrors should provide release-compatible Ruby archive assets and a `SHA256SUMS` asset in the same release download location.
Only exact HTTPS GitHub repository mirrors (`https://github.com/owner/repo`, optionally with port `443`) receive `GITHUB_TOKEN`; other mirrors are used without GitHub authentication.
See [Ruby language support](../languages.md#ruby) for details.

### `PREK_CONDA_INSTALLER`

Select the preinstalled tool used to create environments for `language: conda`
hooks. Supported values are `auto`, `pixi`, `micromamba`, `mamba`, and `conda`.
The default is `auto`, which searches for `pixi`, `micromamba`, `mamba`, then
`conda`. This setting only affects newly created environments; existing matching
environments are reused without requiring the selected installer to remain
available. prek does not install these tools. See
[Conda language support](../languages.md#conda) for details.

### `PREK_RUST_PROFILE`

Override the `rustup` profile used when installing managed Rust toolchains (`minimal`, `default`, or `complete`). Defaults to `minimal`. Set to `default` to include `rustfmt` and `clippy`.
See [Rust language support](../languages.md#rust) for details.

### `PREK_USE_CARGO_BINSTALL`

Use a preinstalled [`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall) for crates.io `cli:` dependencies of `language: rust` hooks.
prek does not install cargo-binstall or override its telemetry and strategy settings. See [Rust language support](../languages.md#rust) for installations that continue to use Cargo.

## Compatibility fallbacks

### `PRE_COMMIT_ALLOW_NO_CONFIG`

Fallback for `PREK_ALLOW_NO_CONFIG`.

### `PRE_COMMIT_NO_CONCURRENCY`

Fallback for `PREK_NO_CONCURRENCY`.

### `SKIP`

Fallback for `PREK_SKIP`.

## Variables exposed to hooks

prek exports `PRE_COMMIT=1` to hook processes. It also exports the following
variables when the selected Git stage supplies the corresponding value:

| Variable | When it is available |
| -- | -- |
| `PRE_COMMIT_FROM_REF`, `PRE_COMMIT_ORIGIN` | The starting ref for revision-range and push-style runs |
| `PRE_COMMIT_TO_REF`, `PRE_COMMIT_SOURCE` | The destination ref for revision-range and push-style runs |
| `PRE_COMMIT_COMMIT_MSG_SOURCE` | The commit-message source supplied by Git |
| `PRE_COMMIT_COMMIT_OBJECT_NAME` | The commit object supplied to `prepare-commit-msg` |
| `PRE_COMMIT_PRE_REBASE_UPSTREAM`, `PRE_COMMIT_PRE_REBASE_BRANCH` | `pre-rebase` arguments |
| `PRE_COMMIT_LOCAL_BRANCH`, `PRE_COMMIT_REMOTE_BRANCH` | `pre-push` branch values |
| `PRE_COMMIT_REMOTE_NAME`, `PRE_COMMIT_REMOTE_URL` | `pre-push` remote values |
| `PRE_COMMIT_CHECKOUT_TYPE` | The checkout flag supplied to `post-checkout` |
| `PRE_COMMIT_SQUASH_MERGE` | Set to `1` for a squash merge |
| `PRE_COMMIT_REWRITE_COMMAND` | The command supplied to `post-rewrite` |

These values describe the current hook invocation. Hooks should tolerate a
variable being absent when they also support stages where Git does not provide
that value.

## Related external variables

prek also honors variables owned by Git, ecosystem tools, or its HTTP stack:

| Variable | Effect |
| -- | -- |
| `SSL_CERT_FILE`, `SSL_CERT_DIR` | Add certificate locations used by HTTPS requests |
| `GITHUB_TOKEN` | Authenticate supported GitHub API requests, including self-update and an exact GitHub Ruby mirror |
| `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY` and lowercase forms | Configure inherited network proxy behavior |

Language installers also inherit relevant ecosystem variables, such as `UV_*`
for Python hook setup. Review unexpected ambient variables when installation
behavior differs between a developer machine and CI.
