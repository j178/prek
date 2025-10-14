# TODO: Parity with pre-commit

This page tracks gaps that prevent `prek` from being a drop-in replacement for `pre-commit`.

## Subcommands not implemented

- `gc`

## Languages not supported yet

The original pre-commit supports hooks written in 10+ languages. Besides the commonly used ones like `python`, `node`, `golang`, `system`, `script`, `pygrep`, `docker`, `docker_image`, and `fail` that are already supported, support for the following languages is still in the works:

- `conda`
- `coursier`
- `dart`
- `dotnet`
- `haskell`
- `julia`
- `lua`
- `perl`
- `r`
- `ruby`
- `rust`
- `swift`

Tracking / issue links:

- Conda: [#52](https://github.com/j178/prek/issues/52)
- Coursier: [#53](https://github.com/j178/prek/issues/53)
- Dart: [#51](https://github.com/j178/prek/issues/51)
- Dotnet: [#48](https://github.com/j178/prek/issues/48)
- Haskell: (no tracking issue yet)
- Julia: (no tracking issue yet)
- Lua: [#41](https://github.com/j178/prek/issues/41)
- Perl: (no tracking issue yet)
- R: [#42](https://github.com/j178/prek/issues/42)
- Ruby: [#43](https://github.com/j178/prek/issues/43)
- Rust: [#44](https://github.com/j178/prek/issues/44)
- Swift: [#46](https://github.com/j178/prek/issues/46)

Contributions welcome — if you'd like to help add support for any of these languages, please open a PR or comment on the corresponding issue.
