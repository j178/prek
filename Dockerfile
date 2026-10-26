FROM --platform=$BUILDPLATFORM docker.io/astral/uv:0.13.4@sha256:9f130421e930bfbdc563c00740466603f8723158e90297d07957c2294640262b AS uv

FROM --platform=$BUILDPLATFORM ubuntu:26.04@sha256:c098726dd9b9733f3e885f8de0ef375206a311b0a317eadc40d347b1115fb944 AS build

# Keep the snapshot at least as recent as the Ubuntu base image.
ARG UBUNTU_SNAPSHOT=20260928T000000Z
# renovate: datasource=github-tags depName=rust-lang/rustup
ARG RUSTUP_VERSION=1.29.2

ENV HOME="/root"
WORKDIR $HOME

# Retry apt downloads to handle transient mirror failures.
RUN echo 'Acquire::Retries "3";' > /etc/apt/apt.conf.d/80-retries

# Install dependencies from an Ubuntu snapshot for reproducibility.
# All build dependencies are in main, so skip the other component indexes.
RUN --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
  sed -i 's/^Components:.*/Components: main/' /etc/apt/sources.list.d/ubuntu.sources \
  && apt install -y --update --no-install-recommends ca-certificates \
  && apt install -y --update --snapshot "${UBUNTU_SNAPSHOT}" --no-install-recommends \
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
  cargo zigbuild --bin prek --release --target $(cat rust_target.txt)
RUN cp target/$(cat rust_target.txt)/release/prek /prek
# TODO: Optimize binary size, with a version that also works when cross compiling
# RUN strip --strip-all /prek

FROM scratch
COPY --from=build /prek /
WORKDIR /io
ENTRYPOINT ["/prek"]
