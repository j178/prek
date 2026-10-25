# prek-ignore

The internal directory walker used by prek's workspace discovery. Prek depends
on this crate by path. It is maintained and versioned with this workspace.

## Source

Forked from the `crates/ignore` directory in
[`BurntSushi/ripgrep` at `6892bc2b7efe5909cbbeec8f757ed2c4c76b1d30`](https://github.com/BurntSushi/ripgrep/tree/6892bc2b7efe5909cbbeec8f757ed2c4c76b1d30/crates/ignore).

The original dual license is preserved in [LICENSE-MIT](LICENSE-MIT) and
[UNLICENSE](UNLICENSE). `globset` remains a crates.io dependency.

## Changes from upstream

- Renamed the crate to `prek-ignore` and integrated it into the prek workspace.
- Replaced the general walker API with `walk(root, callback)` and `WalkState`.
  The callback exposes borrowed, unfiltered directory
  listings so prek can discover configuration files without probing every
  filename. The custom ignore filename is fixed to `.prekignore`.
- Restricted traversal to directories. Ordinary files and child symlinks are
  excluded before constructing paths, matching ignore rules or scheduling work.
- Removed serial iteration, file-type definitions, override globs, incremental
  matchers, multiple roots, sorting, depth and file-size limits, stdin, stdout
  exclusion, symlink following and same-filesystem filtering.
- Removed the matcher option switches, explicit ignore-file API and parent
  matcher cache used by multiple roots. The retained ignore behavior uses the
  defaults that prek previously selected.
- Reused the workspace's Rayon scheduler for parallel traversal, task completion
  and panic propagation. A cancellation flag skips queued work after `Quit` or
  a panic, without maintaining a separate work queue or worker wakeup protocol.
- Share ignore state through directories with no rules. Only new rules or a
  repository boundary create a node. Global rules and root path translation
  belong to the walk rather than each directory node.
- Match only directories and retain just the compiled globs and their whitelist
  flags. Traversal tasks supply reusable matching buffers. Pruned and leaf
  directories do not build matchers.
- Reused the workspace's `regex` and `tracing` dependencies in place of direct
  `regex-automata` and `log` dependencies. `crossbeam-deque` is no longer a direct
  dependency; those crates may still appear as transitive dependencies.
- Use direct ignore-file probes when a directory listing is incomplete, since
  a partial listing cannot establish that a file is absent.
- Reduced error types and dependencies to the retained behavior, adopted the
  workspace's formatting and lint rules, and kept matching regression tests
  together with focused tests of the directory traversal contract.

## Retained behavior

Ignore precedence remains custom ignore file, `.ignore`, `.gitignore`, Git's
`info/exclude`, then global Git excludes. Nested rules, parent rules, Git/Jujutsu
repository boundaries, linked-worktree excludes, hidden entries and explicit
unignore rules retain the upstream behavior.

Each directory is visited before its children. A complete listing includes
files, hidden entries and ignored entries, so a configuration file can still be
found even when it would be excluded from traversal. An unreadable or incomplete
listing is represented by `None`, allowing prek's configuration probes to fall
back to the filesystem. `Skip` prunes children and `Quit` cancels the walk;
callbacks already running may still complete. Read errors do not stop discovery,
and valid rules from partially invalid ignore files still apply.

Changes to matching semantics should be checked against the source commit and
prek's workspace tests. This crate exposes only the traversal interface needed
by prek; general-purpose upstream APIs should not be restored without a caller.

## Release integration

The path dependency also specifies the workspace version so the existing
`cargo publish --workspace` workflow can publish prek and its dependencies.

Before the first release from this repository, configure the existing
`prek-ignore` package's crates.io trusted publisher for repository `j178/prek`,
workflow `publish-crates.yml`, environment `release`, matching
[the workspace publishing workflow](https://github.com/j178/prek/blob/main/.github/workflows/publish-crates.yml).
