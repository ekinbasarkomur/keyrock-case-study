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

# --- dependency layer -------------------------------------------------------
# Copy ONLY the manifests first and build against a stub source tree, so the
# (slow) dependency compile is cached and does not repeat on every source edit.
COPY Cargo.toml Cargo.lock ./
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
    && cargo build --release \
    && strip target/release/keyrock-case-study

# --- runtime ----------------------------------------------------------------
FROM debian:bookworm-slim

# ca-certificates is the one thing a networked Rust binary almost always needs
# and the slim image does not ship; without it every TLS call fails with an
# unhelpful certificate error. Apt lists are removed in the same layer so they
# do not end up in the image.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Non-root, matching the uid used by the other services in this workspace.
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
CMD ["--help"]
