FROM --platform=$BUILDPLATFORM ghcr.io/astral-sh/uv:0.11.28@sha256:0f36cb9361a3346885ca3677e3767016687b5a170c1a6b88465ec14aefec90aa AS uv

FROM --platform=$BUILDPLATFORM ubuntu:24.04@sha256:4fbb8e6a8395de5a7550b33509421a2bafbc0aab6c06ba2cef9ebffbc7092d90 AS build

ARG UBUNTU_SNAPSHOT=20260301T000000Z
ARG RUSTUP_VERSION=1.28.2

ENV HOME="/root"
WORKDIR $HOME

# Retry apt downloads to handle transient mirror failures.
RUN echo 'Acquire::Retries "3";' > /etc/apt/apt.conf.d/80-retries

# Install dependencies from an Ubuntu snapshot for reproducibility.
RUN --mount=type=cache,target=/var/lib/apt/lists \
  apt install -y --update ca-certificates \
  && apt install -y --update --snapshot ${UBUNTU_SNAPSHOT} --no-install-recommends \
  build-essential \
  curl

# Install uv
COPY --from=uv /uv /usr/local/bin/uv

# Setup zig as cross compiling linker
COPY pyproject.toml uv.lock ./
RUN uv sync --only-group docker --locked
ENV PATH="$HOME/.venv/bin:$PATH"

# Install rust
ARG TARGETPLATFORM
RUN case "$TARGETPLATFORM" in \
  "linux/arm64") echo "aarch64-unknown-linux-musl" > rust_target.txt ;; \
  "linux/amd64") echo "x86_64-unknown-linux-musl" > rust_target.txt ;; \
  *) exit 1 ;; \
  esac

# Install a pinned rustup release.
RUN curl --proto '=https' --tlsv1.2 -sSf \
  "https://static.rust-lang.org/rustup/archive/${RUSTUP_VERSION}/$(uname -m)-unknown-linux-gnu/rustup-init" \
  -o rustup-init \
  && chmod +x rustup-init \
  && ./rustup-init -y --target $(cat rust_target.txt) --profile minimal --default-toolchain none \
  && rm rustup-init
ENV PATH="$HOME/.cargo/bin:$PATH"

# Install the toolchain in the musl target
COPY rust-toolchain.toml rust-toolchain.toml
RUN rustup toolchain install
RUN rustup target add $(cat rust_target.txt)

# Build
COPY ./Cargo.toml Cargo.toml
COPY ./Cargo.lock Cargo.lock
COPY crates crates
RUN case "${TARGETPLATFORM}" in \
  "linux/arm64") export JEMALLOC_SYS_WITH_LG_PAGE=16;; \
  esac && \
  cargo zigbuild --bin prek --profile dist --target $(cat rust_target.txt)
RUN cp target/$(cat rust_target.txt)/dist/prek /prek
# TODO: Optimize binary size, with a version that also works when cross compiling
# RUN strip --strip-all /prek

FROM scratch
COPY --from=build /prek /
WORKDIR /io
ENTRYPOINT ["/prek"]
