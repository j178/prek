# Integrations

This page documents common ways to integrate prek into CI and container workflows.

## Docker

prek is published as a distroless container image at:

- `ghcr.io/j178/prek`

The image is based on `scratch` (no shell, no package manager). It contains the prek binary at `/prek`.

A common pattern is to copy the binary into your own image:

```dockerfile
FROM debian:bookworm-slim
COPY --from=ghcr.io/j178/prek:v0.2.28 /prek /usr/local/bin/prek
```

If you prefer, you can also run the distroless image directly:

```bash
docker run --rm ghcr.io/j178/prek:v0.2.28 --version
```

## GitHub Actions

{%
  include-markdown "../README.md"
  start="<!-- github-actions:start -->"
  end="<!-- github-actions:end -->"
%}
