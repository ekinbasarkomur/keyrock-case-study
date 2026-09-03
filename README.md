# rust-crypto-orderbook

A Rust order book aggregator. The finished service connects to Binance and Bitstamp order-book websocket
feeds, merges the two books for one traded pair, and streams the spread plus
the top 10 bids/asks over gRPC (`proto/orderbook.proto`).

**Steps 0-3 of an 11-step build order have landed.** The binary connects to
Binance's `depth20@100ms` websocket stream, parses each snapshot into the
internal `Book` type, and sends it down an `mpsc` channel to an aggregator
task. The aggregator task summarises the book and publishes it into a
`watch` channel that a gRPC server (`src/server.rs`) streams to clients as
a `Summary` — spread, top 10 bids, top 10 asks — over `tonic`, on every
change. **This is real Binance market data end to end**: every streamed
`Level.exchange` reads `"binance"`, not a placeholder. There is still no
second venue (Bitstamp), no merge logic (this step summarises one book
only), and no reconnection/staleness handling — those are later steps.
Later steps build on this without reshaping it.

## Build order

| Step | What it is | Status |
| --- | --- | --- |
| 0 | Scaffold: dependencies, proto, `build.rs`, CLI, config, Docker | Done |
| 1 | Binance feed: connect, parse, print | Done |
| 2 | gRPC server streaming a static/fake `Summary` | Done |
| 3 | Wire step 1 into step 2 — first real end-to-end milestone | Done |
| 4 | Add the Bitstamp feed | Not started |
| 5 | Real `merge()` + top-10 + spread — the core deliverable | Not started |
| 6 | Reconnection + staleness handling | Not started |
| 7 | Remaining edge cases | Not started |
| 8 | Latency measurement (p50/p99), written up here | Not started |
| 9 | Add the `client` binary to `compose.yml` | Not started |
| 10 | README pass, final cleanup | Not started |

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

Both parse arguments, build a `Config`, log a `starting` line to stderr,
then run three concurrent tasks: the Binance feed (connects, parses each
`depth20` update, logs it to stderr, and sends it down an `mpsc` channel),
the aggregator (receives from that channel, summarises the book, and
publishes into a `watch` channel), and the gRPC server (listens on
`--port`, defaulting to `50051`, and streams the `watch` channel's value to
every subscriber). All logging goes to stderr; stdout stays empty. The
process exits the moment any one of the three tasks ends — see "gRPC
server" below.

The feed-to-aggregator `mpsc` channel is bounded at 32 rather than
unbounded — an unbounded channel would hide a stuck aggregator behind
unlimited memory growth instead of surfacing backpressure.

## gRPC server

`src/server.rs` implements the `OrderbookAggregator` service from
`proto/orderbook.proto` and streams a `Summary` over a
`tokio::sync::watch` channel, forwarding a new value to every subscriber
whenever the aggregator publishes one. **The data is real Binance market
data, end to end**: `src/aggregator.rs` receives each parsed `Book` from
the feed over an `mpsc` channel, calls the pure `summarise()` in
`src/merge.rs`, and publishes the result into the `watch` channel this
server streams from. Every `Level.exchange` reads `"binance"`. Verified
live against `stream.binance.com` (see "gRPC server" verification below,
and Docker section) — `grpcurl` output looks like plausible real ETHBTC
market data, not a static repeated value:

```
$ grpcurl -plaintext -max-time 3 127.0.0.1:50051 orderbook.OrderbookAggregator/BookSummary
{
  "spread": 1.0000000000003062e-05,
  "bids": [
    { "exchange": "binance", "price": 0.03158, "amount": 19.1949 },
    { "exchange": "binance", "price": 0.03157, "amount": 91.9584 },
    ...
  ],
  "asks": [
    { "exchange": "binance", "price": 0.03159, "amount": 78.5329 },
    ...
  ]
}
```

There is still only one venue (Binance) — merging a second venue's book
and real spread computation across both is step 5.

`tonic-reflection` is registered alongside the aggregator service, so a
client can discover the schema without a local `.proto` file:

```sh
grpcurl -plaintext localhost:50051 list
# grpc.reflection.v1.ServerReflection
# orderbook.OrderbookAggregator

grpcurl -plaintext -max-time 3 localhost:50051 orderbook.OrderbookAggregator/BookSummary
```

**Reflection is unconditionally enabled in this build** — convenient for a
reviewer exploring the service with no client code of their own, but it
exposes the entire schema to anything that can reach the port. A
production-facing deployment should gate it behind a feature flag or
config toggle; this repo does not.

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
│   ├── proxy.rs          parses HTTP_PROXY/HTTPS_PROXY and implements the
│   │                     optional HTTP CONNECT tunnel
│   ├── exchange/
│   │   ├── mod.rs        declares the binance submodule and the Venue enum
│   │   │                 (Binance only for now) — no Exchange trait yet,
│   │   │                 see specs/002-binance-feed/spec.md
│   │   └── binance.rs    connect URL, read loop, parse() -> Option<Book>
│   ├── merge.rs          pure summarise(venue, book) -> Option<Summary> —
│   │                     no clock, no I/O, no channel; the entry point
│   │                     step 5 widens into a real two-book merge()
│   ├── aggregator.rs     owns the last-seen Book per venue as task-local
│   │                     state (fed over the mpsc from the feed), drives
│   │                     summarise(), and publishes into the watch channel
│   ├── server.rs         OrderbookAggregator gRPC service, reflection, and
│   │                     the watch-channel-to-stream plumbing
│   └── main.rs           CLI entry point — parses arguments, spawns the
│                         feed, aggregator, and gRPC server tasks under
│                         one select!
└── tests/
    ├── cli.rs            integration tests: the real binary, as a subprocess
    └── grpc.rs           integration test: real server, real tonic client,
                          asserts on two streamed Summary messages
```

The library/binary split is intentional: integration tests under `tests/`
can only import a library crate, so logic in `main.rs` wouldn't be
reachable from them.

## Configuration

Every setting can be given two ways: a CLI flag or a `ORDERBOOK_`-prefixed
environment variable, both with working defaults — the binary runs with no
flags and no environment at all.

| Setting | Flag | Env var | Default | Meaning |
| --- | --- | --- | --- | --- |
| Pair | `--pair` | `ORDERBOOK_PAIR` | `ethbtc` | Traded pair the Binance feed subscribes to; the gRPC server streams this pair's real book as of step 3. |
| Port | `--port` | `ORDERBOOK_PORT` | `50051` | Port the gRPC server binds. An unparseable `ORDERBOOK_PORT` is a startup error. |
| Log level | — | `ORDERBOOK_LOG_LEVEL` | `info` | `RUST_LOG`-style filter; an explicit `RUST_LOG` wins over this. |
| Host | — | `ORDERBOOK_HOST` | `127.0.0.1` | Bind address for the gRPC server. Use `0.0.0.0` in a container. |

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
to start. Any HTTP `CONNECT` proxy works, provided port 9443 is allowed
through its `SSL_ports`/`Safe_ports` ACL.

**Verified live** (2026-08-23, via the proxy path above): the connection
held for 27 minutes straight against Binance's 20s ping / 60s pong-timeout —
84 pings received, 84 pongs sent automatically by `tokio-tungstenite`, 3,079
book updates logged, zero errors. Full evidence in
`specs/002-binance-feed/revisions.md` entry 5.

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
docker compose up --build                        # defaults, stays running, serves gRPC on 127.0.0.1:50051
docker compose run --rm app --pair btcusd --port 12345
```

Two-stage build: `rust:1.97-slim-bookworm` compiles the binary, a slim
`debian:bookworm-slim` image runs it as a non-root user. The builder stage
installs `protobuf-compiler` itself — no host setup required. The runtime
stage skips `ca-certificates`: `tokio-tungstenite`'s
`rustls-tls-webpki-roots` feature bundles its own root certs.

The container binds `0.0.0.0` internally (`ORDERBOOK_HOST=0.0.0.0`, set in
`compose.yml`) and publishes `127.0.0.1:50051` on the host, loopback only.
With the container able to reach Binance (directly, or via the
`PROXY_HOST`/`PROXY_PORT` pass-through in `.env` — see Configuration
above), `docker compose up` stays running and `grpcurl -plaintext
localhost:50051 list` from the host works exactly as in the gRPC server
section above. Verified live in this repo's own dev environment:

```
$ grpcurl -plaintext localhost:50051 list
grpc.reflection.v1.ServerReflection
orderbook.OrderbookAggregator
```

**If the container can't reach Binance and no proxy is configured, it
exits shortly after starting.** This is the `select!` supervision in
`main.rs` working as designed, not a defect: the moment the feed task ends
(connection refused, no route, etc.), the whole process exits rather than
leaving a gRPC server answering with data from a feed that's already dead.
If you see the container start then stop, check whether your network can
reach `stream.binance.com` directly, and if not, set `PROXY_HOST`/
`PROXY_PORT` in `.env` (see Configuration above).

## Deployment notes

The service is a single stateless container — one process per pair, no
persistent state — so it scales horizontally and restarts cleanly with no
recovery step. Binance is unreachable from Turkish networks, so development
runs through a CONNECT proxy — a `t3.nano` Squid instance in `eu-central-1`,
sized for a single websocket. Any CONNECT proxy works, provided port 9443 is
allowed through its `SSL_ports`/`Safe_ports` ACL. In a real deployment,
egress would go through a NAT gateway or VPC endpoint rather than a
standalone instance, and the container would carry a real health check
rather than relying on `select!`'s exit-on-failure as the only signal.

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
