# syntax=docker/dockerfile:1
# Cross-compiles natively on the build host (no emulation), e.g.
#   docker build --platform linux/amd64 -t hydra-panel:latest .
FROM --platform=$BUILDPLATFORM rust:1-slim-trixie AS build
ARG TARGETARCH
ARG BUILDARCH
RUN set -eu; \
    case "$TARGETARCH" in \
      amd64) t=x86_64-unknown-linux-gnu; gcc=x86-64 ;; \
      arm64) t=aarch64-unknown-linux-gnu; gcc=aarch64 ;; \
      *) echo "unsupported $TARGETARCH"; exit 1 ;; \
    esac; \
    echo "$t" > /rust-target; \
    if [ "$TARGETARCH" != "$BUILDARCH" ]; then \
      apt-get update && apt-get install -y --no-install-recommends "gcc-$gcc-linux-gnu" "libc6-dev-$TARGETARCH-cross" && rm -rf /var/lib/apt/lists/*; \
    fi; \
    rustup target add "$t"
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
COPY static static
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target,id=hydra-panel-target-$TARGETARCH \
    t="$(cat /rust-target)" && \
    cargo build --release --locked --target "$t" && \
    cp "target/$t/release/hydra-panel" /hydra-panel

FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=build /hydra-panel /hydra-panel
EXPOSE 8080
ENTRYPOINT ["/hydra-panel"]
