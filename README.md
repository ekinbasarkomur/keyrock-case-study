# keyrock-case-study

A take-home case study for a Rust engineer application at Keyrock. The
finished service connects to Binance and Bitstamp order-book websocket
feeds, merges the two books for one traded pair, and streams the spread plus
the top 10 bids/asks over gRPC (`proto/orderbook.proto`).

**None of that logic exists yet.** This is step 0 of an 11-step build order:
a containerised skeleton with the dependencies the build is expected to
need, a real CLI, a real config system, and a proto build pipeline whose
output is actually compiled — but no websocket client, merge logic, or gRPC
service yet. Later steps build on this without reshaping it.

## Requirements

- Rust 1.97.1 — `rust-toolchain.toml` pins it, `rustup` installs it
  automatically.
- `protoc` for host builds — `build.rs` needs it to compile
  `proto/orderbook.proto`. `apt install protobuf-compiler` / `brew install
  protobuf`.
- Docker (optional — the image installs `protoc` itself, no host setup
  needed if you only run it in a container).

## Quick start

```sh
cargo run --                            # defaults: --pair ethbtc --port 50051
cargo run -- --pair btcusd --port 12345

docker compose up --build               # same thing, containerised
```

Both parse arguments, build a `Config`, log one `starting` line to stderr,
and exit 0. There's no server yet, so nothing listens on the port and
stdout stays empty.

## Development

```sh
cargo test                                  # unit + integration
cargo clippy --all-targets -- -D warnings   # lints are errors
cargo fmt --check
```

## Layout

```
.
├── proto/
│   └── orderbook.proto   the gRPC schema — copied verbatim from the brief,
│                         never hand-edited
├── build.rs              compiles proto/orderbook.proto to Rust via
│                         tonic-prost-build
├── src/
│   ├── lib.rs            library root; also pulls in the generated proto
│   │                     types via tonic::include_proto! so the build
│   │                     pipeline is proven, not just assumed
│   ├── config.rs         configuration, read from the environment
│   ├── telemetry.rs      tracing setup (logs to stderr)
│   ├── model.rs          Price/Amount newtypes and Book — exchange-agnostic
│   ├── proxy.rs          parses HTTP_PROXY/HTTPS_PROXY for the optional
│   │                     CONNECT tunnel
│   ├── exchange/
│   │   ├── mod.rs        declares the binance submodule — no trait yet,
│   │   │                 see specs/002-binance-feed/spec.md
│   │   └── binance.rs    connect URL, read loop, parse() -> Option<Book>
│   └── main.rs           CLI entry point — parses arguments, connects,
│                         drives the read loop
└── tests/
    └── cli.rs            integration tests: the real binary, as a subprocess
```

The library/binary split is intentional: integration tests under `tests/`
can only import a library crate, so logic in `main.rs` wouldn't be
reachable from them.

## Configuration

Every setting can be given two ways: a CLI flag or a `KEYROCK_`-prefixed
environment variable, both with working defaults — the binary runs with no
flags and no environment at all.

| Setting | Flag | Env var | Default | Meaning |
| --- | --- | --- | --- | --- |
| Pair | `--pair` | `KEYROCK_PAIR` | `ethbtc` | Traded pair, once feed/merge logic lands. |
| Port | `--port` | `KEYROCK_PORT` | `50051` | Port the service will bind, once the gRPC server exists. An unparseable `KEYROCK_PORT` is a startup error. |
| Log level | — | `KEYROCK_LOG_LEVEL` | `info` | `RUST_LOG`-style filter; an explicit `RUST_LOG` wins over this. |
| Host | — | `KEYROCK_HOST` | `127.0.0.1` | Bind address for the eventual server. Use `0.0.0.0` in a container. |

**A CLI flag overrides its matching env var when both are given.**
`--pair`/`--port` are the only two settings exposed as flags; log level and
host are env-var only.

Copy `.env.example` to `.env` to set the env vars without exporting them by
hand.

Logs go to stderr so stdout stays pipeable.

If `HTTPS_PROXY` (or `HTTP_PROXY`) is set, the Binance connection tunnels
through it via an HTTP `CONNECT` handshake instead of dialing Binance
directly — useful on networks that can't reach `stream.binance.com`
directly. Unset, behavior is unchanged. `compose.yml` builds this from
`PROXY_HOST`/`PROXY_PORT` in `.env` (see `.env.example`); a malformed proxy
value falls back to a direct connection with a warning rather than failing
to start.

## Price representation

`Price`/`Amount` are thin newtypes over `f64` — chosen for type safety (the
compiler rejects passing an `Amount` where a `Price` goes) and a
well-defined total order (`Ord` via `f64::total_cmp`) for sorting, not for
raw precision. Fixed-point was tried first and measured against two million
realistic ETHBTC price pairs before being dropped as unneeded complexity at
this scale (detail in `specs/002-binance-feed/revisions.md`). The one
computed value, the combined-book spread (step 5), is rounded to 8 decimals
at the gRPC boundary. A system whose arithmetic accumulates over many
updates — unlike this one — would be right to reach for integers instead.

## Docker

```sh
docker compose up --build                        # defaults, logs, exits 0
docker compose run --rm app --pair btcusd --port 12345
```

Two-stage build: `rust:1.97-slim-bookworm` compiles the binary, a slim
`debian:bookworm-slim` image runs it as a non-root user. The builder stage
installs `protobuf-compiler` itself — no host setup required. The runtime
stage skips `ca-certificates`: `tokio-tungstenite`'s
`rustls-tls-webpki-roots` feature bundles its own root certs.

No long-running service yet, so `docker compose up --build` logs the
startup line and exits 0 rather than staying up. A later step adds a
`command:`, a `ports:` publish on loopback, and a `HEALTHCHECK`.

## What would change for production

**Pair selection belongs on the request, not the process.** `BookSummary`
takes `Empty` — the client can't ask for a pair, so it's fixed at startup
and a second pair means a second process. Fine for one pair, but it doesn't
scale by multiplexing: Binance alone allows up to 1024 streams per
connection, so a few hundred pairs under this model burns a few hundred
connections and processes to do work one connection could plausibly do. A
production schema would put the pair on the request message and let one
instance fan a single upstream connection across many books — noted here
rather than edited into `proto/orderbook.proto`, since the given schema is
treated as fixed per the brief.
