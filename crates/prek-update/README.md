# prek-update

Self-update support for prek installations created by the standalone cargo-dist
installers.

Adapted from [axoupdater 0.10.2](https://github.com/axodotdev/axoupdater/tree/a08bbbdc06dfb2f18c1a55eb780e042f12fcd7d4),
under its MIT license. The original license is retained in
`LICENSE.axoupdater`.

This crate keeps receipt discovery and validation, GitHub release selection,
installer execution, and Windows replacement recovery. It uses the caller's HTTP
client and existing workspace dependencies instead of the general axo libraries.
