# keyrock-case-study

A take-home case study for a Rust engineer application at Keyrock. The
finished service connects to Binance and Bitstamp order-book websocket
feeds, merges the two books for one traded pair, and streams the spread plus
the top 10 bids/asks over gRPC (`proto/orderbook.proto`).

**None of that logic exists yet.** This is step 0 of an 11-step build order:
a dependency-complete, containerised skeleton with a real CLI, a real config
system, and a working proto build pipeline, but no websocket client, no
merge logic, and no gRPC service. Later steps build on this without
reshaping it.

## Requirements

- Rust 1.97.1 — `rust-toolchain.toml` pins it, and `rustup` will install it
  automatically on first build.
- `protoc` (the Protocol Buffers compiler) for the host build — `build.rs`
  shells out to it via `tonic-prost-build` to compile `proto/orderbook.proto`.
  On Debian/Ubuntu: `apt install protobuf-compiler`. On macOS: `brew install
  protobuf`.
- Docker (optional, for the container path — the image installs `protoc`
  itself, see "Docker" below).

## Quick start

```sh
cargo run --                            # defaults: --pair ethbtc --port 50051
cargo run -- --pair btcusd --port 12345
```

Both invocations parse arguments, build a `Config`, log one `starting`
line to stderr, and exit 0. There is no server yet, so nothing is listening
on the port and stdout stays empty — the port is just what's recorded for
now, ahead of the gRPC server that will actually bind it in a later step.

## Development

```sh
cargo test                                  # unit + integration
cargo clippy --all-targets -- -D warnings   # lints are errors
cargo fmt --check
```

## Layout

```
proto/orderbook.proto  the gRPC schema — copied verbatim from the brief,
                        never hand-edited (see "What would change for
                        production" below for any opinion about it)
build.rs                compiles proto/orderbook.proto to Rust via
                        tonic-prost-build; nothing consumes the generated
                        types yet — that lands with the gRPC server step
src/lib.rs              library root — everything testable lives here
src/config.rs           configuration, read from the environment
src/telemetry.rs        tracing setup (logs to stderr)
src/main.rs             CLI entry point — parses arguments and delegates
tests/cli.rs            integration tests: the real binary, as a subprocess
```

The library/binary split is intentional: integration tests under `tests/` can
only import a library crate, so logic placed directly in `main.rs` would not be
reachable from them.

## Configuration

Every setting can be given two ways: a CLI flag or a `KEYROCK_`-prefixed
environment variable. Both have working defaults, so the binary runs with no
flags and no environment at all.

| Setting | Flag | Env var | Default | Meaning |
| --- | --- | --- | --- | --- |
| Pair | `--pair` | `KEYROCK_PAIR` | `ethbtc` | Traded pair the aggregator will stream once feed/merge logic lands. |
| Port | `--port` | `KEYROCK_PORT` | `50051` | Port the service will bind to once the gRPC server exists. An unparseable `KEYROCK_PORT` is a startup error, not a silent fallback. |
| Log level | — | `KEYROCK_LOG_LEVEL` | `info` | `RUST_LOG`-style filter. An explicit `RUST_LOG` in the environment wins over this. |
| Host | — | `KEYROCK_HOST` | `127.0.0.1` | Bind address for the eventual server. Use `0.0.0.0` inside a container. |

**Precedence: a CLI flag overrides its matching environment variable when
both are given.** `--pair`/`--port` are the more specific, closer-to-the-call-site
input, so `Config::from_env()` runs first to resolve the env-or-default value,
then any flag that was actually passed overwrites it. `--pair` and `--port` are
the only two settings exposed as flags; log level and host are env-var only.

Copy `.env.example` to `.env` to set the env vars without exporting them by
hand.

Logs are written to stderr so stdout carries only program output and stays
pipeable.

## Docker

```sh
docker compose up --build                        # runs with defaults, logs, exits 0
docker compose run --rm app --pair btcusd --port 12345
```

Two-stage build: a full `rust:1.97-slim-bookworm` toolchain compiles the
binary, and a slim `debian:bookworm-slim` image runs it as a non-root user.
The builder stage installs `protobuf-compiler` via apt — `tonic-prost-build`
shells out to `protoc` and does not bundle it, so the image needs the same
tool the host build needs. The runtime stage installs no `ca-certificates`
package: `tokio-tungstenite`'s `rustls-tls-webpki-roots` feature bundles its
own root certificates, so TLS works without a system CA store once the
websocket clients land, and there's no reason to carry a package the design
has already committed to not needing.

There is no long-running service yet, so `docker compose up --build` starts
the container, logs the one startup line, and exits 0 rather than staying up.
When a server lands in a later step, this file gains a `command:`, a
`ports:` publish on loopback, and a `HEALTHCHECK`.

## What would change for production

<!-- Placeholder — nothing schema-related has come up yet at this step. Any
     opinion about proto/orderbook.proto's shape belongs here, never as an
     edit to the .proto file itself, since the reviewer tests against their
     own copy of it. -->
