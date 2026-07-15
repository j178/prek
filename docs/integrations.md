# Integrations

This page documents common ways to integrate prek into CI and container workflows.

## Docker

prek publishes container images under `ghcr.io/j178/prek`:

| Tags | Base | Contents |
| -- | -- | -- |
| `X.Y.Z`, `X.Y`, `latest` | `scratch` | The `prek` binary only |
| `X.Y.Z-alpine`, `X.Y-alpine`, `alpine` | Current Alpine release | `prek`, Git, and CA certificates |
| `X.Y.Z-alpine3.24`, `X.Y-alpine3.24`, `alpine3.24` | `alpine:3.24` | Version-pinned Alpine variant |

!!! note

    Docker image tags before `0.4.10` include a leading `v`, for example
    `ghcr.io/j178/prek:v0.4.9`. The Alpine variant is available starting with
    `0.4.10`.

### Minimal (scratch)

The image is based on `scratch` (no shell, no package manager). It contains the prek binary at `/prek`.

A common pattern is to copy the binary into your own image:

```dockerfile
FROM debian:bookworm-slim
COPY --from=ghcr.io/j178/prek:0.4.10 /prek /usr/local/bin/prek
```

If you prefer, you can also run the distroless image directly:

```bash
docker run --rm ghcr.io/j178/prek:0.4.10 --version
```

### Alpine

The Alpine variant includes `prek`, Git, CA certificates, a shell, and the Alpine package manager.

```bash
docker run --rm ghcr.io/j178/prek:0.4.10-alpine --version
```

Use `X.Y.Z-alpine3.24` to pin both the prek and Alpine versions, or `alpine3.24` to pin only the
Alpine version while tracking the latest prek release. Tags without the numbered Alpine suffix,
such as `X.Y.Z-alpine` and `alpine`, use the current supported Alpine release.

### Verifying Images

All Docker image variants are signed with
[GitHub Attestations](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations)
to verify they were built by official prek workflows. Verify using the
[GitHub CLI](https://cli.github.com/):

```console
$ gh attestation verify --owner j178 oci://ghcr.io/j178/prek:latest
Loaded digest sha256:xxxx... for oci://ghcr.io/j178/prek:latest
Loaded 1 attestation from GitHub API
✓ Verification succeeded!

- Attestation #1
  - Build repo:..... j178/prek
  - Build workflow:. .github/workflows/build-docker.yml@refs/tags/vX.Y.Z
```

!!! tip

    Use a specific version tag (e.g., `ghcr.io/j178/prek:0.4.10`) or image
    digest rather than `latest` for verification.

## GitHub Actions

--8<-- "README.md:github-actions"

## prek skill for agents

--8<-- "README.md:gh-skill-install"
