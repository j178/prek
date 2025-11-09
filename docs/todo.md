# TODO: Parity with pre-commit

This page tracks gaps that prevent `prek` from being a drop-in replacement for `pre-commit`.

## Subcommands not implemented

- `gc`

## Language support status

The original pre-commit supports hooks written in 10+ languages. The table below tracks `prek`'s parity progress and references any open follow-up work.

| Language | Status in `prek` | Tracking | Notes |
| --- | --- | --- |  |
| python ⭐ | ✅ Supported | — | `prek` supports automatic version management of Python toolchains. |
| node | ✅ Supported | — | |
| golang | ✅ Supported | — | |
| rust | ✅ Supported | — | |
| lua | ✅ Supported | — | |
| system | ✅ Supported | — | |
| script | ✅ Supported | — | |
| pygrep | ✅ Supported | — | |
| docker | ✅ Supported | — | |
| docker_image | ✅ Supported | — | |
| fail | ✅ Supported | — | |
| deno ⭐ | 🚧 WIP | — | Experimental support in `prek`; upstream `pre-commit` lacks a native `deno` language. |
| ruby | 🚧 WIP | [#43](https://github.com/j178/prek/issues/43) | `prek` does not currently support downloading new Ruby versions, but can use multiple simultaneously installed interpreters |
| conda | 🚧 Planned | [#52](https://github.com/j178/prek/issues/52) | |
| coursier | 🚧 Planned | [#53](https://github.com/j178/prek/issues/53) | |
| dart | 🚧 Planned | [#51](https://github.com/j178/prek/issues/51) | |
| dotnet | 🚧 Planned | [#48](https://github.com/j178/prek/issues/48) | |
| haskell | 🚧 Planned | — | |
| julia | 🚧 Planned | — | |
| perl | 🚧 Planned | — | |
| r | 🚧 Planned | [#42](https://github.com/j178/prek/issues/42) | |
| swift | 🚧 Planned | [#46](https://github.com/j178/prek/issues/46) | |

⭐ Languages marked with a star highlight functionality `prek` offers beyond what upstream `pre-commit` includes today.

Contributions welcome — if you'd like to help add support for any of these languages, please open a PR or comment on the corresponding issue.
