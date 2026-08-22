# keyrock-case-study as a container.
#
# Two stages: a full Rust toolchain builds the binary, and a slim Debian image
# runs it. The toolchain is ~1.5 GB and has no business shipping — the runtime
# image carries the binary and its libc, nothing else.
#
# Both base images are PINNED. `rust:latest` silently changes compiler version
# between builds, which turns "works on my machine" into a version question
# nobody thought to ask. The tag here must stay in sync with
# rust-toolchain.toml, which pins the same version for host builds.

FROM rust:1.97-slim-bookworm AS builder

WORKDIR /build

# protobuf-compiler provides `protoc`, which build.rs shells out to via
# tonic-prost-build. The crate does not bundle it (confirmed empirically:
# the stub build below fails without this line, with prost-build's own
# "Could not find `protoc`" error).
RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# --- dependency layer -------------------------------------------------------
# Copy ONLY the manifests first and build against a stub source tree, so the
# (slow) dependency compile is cached and does not repeat on every source edit.
# build.rs runs on EVERY `cargo build`, including this stub-source compile —
# it is not conditional on src/ being real — so it and the proto/ tree it
# reads must be present here too, or this stub build fails outright.
COPY build.rs Cargo.toml Cargo.lock ./
COPY proto/ ./proto/
RUN mkdir -p src \
    && echo 'fn main() {}' > src/main.rs \
    && echo '' > src/lib.rs \
    && cargo build --release \
    && rm -rf src

# --- application layer ------------------------------------------------------
COPY src/ ./src/

# THE TRAP: Cargo decides what to rebuild from mtime. The stub main.rs above
# and the real one copied here can land in the same second, in which case Cargo
# considers the stale build fresh and ships a binary that just prints nothing.
# `touch` forces the timestamp forward so the real source is always rebuilt.
# This failure is silent — the image builds, runs, and does nothing.
RUN touch src/main.rs src/lib.rs \
    && cargo build --release

# --- runtime ----------------------------------------------------------------
FROM debian:bookworm-slim

# No ca-certificates package here: tokio-tungstenite's rustls-tls-webpki-roots
# feature bundles its own root certs, so TLS works without a system CA store.
# Nothing else in this stage needs apt, so there is no apt block at all.

# Non-root, fixed uid above the typical system-account range.
RUN useradd --create-home --uid 10001 app

WORKDIR /app
COPY --from=builder /build/target/release/keyrock-case-study /usr/local/bin/keyrock-case-study

USER app

# No HEALTHCHECK yet, deliberately: there is no long-running server to probe.
# A healthcheck pointed at a port nothing listens on marks the container
# permanently unhealthy, which trains everyone to ignore the status column.
# When a server lands here, add one then — and give it a --start-period long
# enough to cover startup:
#
#   HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
#       CMD curl -fsS http://127.0.0.1:8080/health || exit 1
#
# (that needs `curl` added to the apt line above — the slim image has none).

ENTRYPOINT ["keyrock-case-study"]
CMD []
