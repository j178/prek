# Integrations

This page documents common ways to integrate prek into CI and container workflows.

## Docker

prek publishes two container images:

| Image | Base | Description |
| -- | -- | -- |
| `ghcr.io/j178/prek` | `scratch` | Minimal distroless image containing only the prek binary |
| `ghcr.io/j178/prek-alpine` | `alpine:3.23` | Alpine image with common hook dependencies pre-installed |

### Minimal (scratch)

The default image is based on `scratch` (no shell, no package manager). It contains the prek binary at `/prek`.

A common pattern is to copy the binary into your own image:

```dockerfile
FROM debian:bookworm-slim
COPY --from=ghcr.io/j178/prek:v0.3.9 /prek /usr/local/bin/prek
```

If you prefer, you can also run the distroless image directly:

```bash
docker run --rm ghcr.io/j178/prek:v0.3.9 --version
```

### Alpine

The Alpine image includes `git`, `nodejs`, `npm`, `python3`, and `py3-pip`, covering the most common hook runtimes.

```bash
docker run --rm ghcr.io/j178/prek-alpine:v0.3.9 --version
```

### Verifying Images

Both images are signed with
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

    Use a specific version tag (e.g., `ghcr.io/j178/prek:v0.3.9`) or image
    digest rather than `latest` for verification.

## GitHub Actions

--8<-- "README.md:github-actions"

## prek skill for agents

--8<-- "README.md:gh-skill-install"
