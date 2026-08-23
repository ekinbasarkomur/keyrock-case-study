# keyrock-case-study

A take-home case study for a Rust engineer application at Keyrock. The
finished service connects to Binance and Bitstamp order-book websocket
feeds, merges the two books for one traded pair, and streams the spread plus
the top 10 bids/asks over gRPC (`proto/orderbook.proto`).

**Steps 0-4 of an 11-step build order have landed.** Both feeds carry real
market data into one aggregator task over a shared `mpsc`, published into a
`watch` channel that `src/server.rs` streams to gRPC clients on every
change. **The published book is still single-venue this step** —
`merge::summarise()` reads only the map's first (lowest-ordered, i.e.
Binance) entry; real two-book merging is step 5. No reconnection/staleness
handling yet — that's step 6.

## Build order

| Step | What it is | Status |
| --- | --- | --- |
| 0 | Scaffold: dependencies, proto, `build.rs`, CLI, config, Docker | Done |
| 1 | Binance feed: connect, parse, print | Done |
| 2 | gRPC server streaming a static/fake `Summary` | Done |
| 3 | Wire step 1 into step 2 — first real end-to-end milestone | Done |
| 4 | Add the Bitstamp feed | Done |
| 5 | Real `merge()` + top-10 + spread — the core deliverable | Not started |
| 6 | Reconnection + staleness handling | Not started |
| 7 | Remaining edge cases | Not started |
| 8 | Latency measurement (p50/p99), written up here | Not started |
| 9 | Add the `client` binary to `compose.yml` | Not started |
| 10 | README pass, final cleanup | Not started |

The aggregator holds a `BTreeMap<Venue, VenueState>` rather than one field
per venue, so adding a venue never forces a `merge(a)` → `merge(a, b)`
signature change.

## Requirements

- Rust 1.97.1 — `rust-toolchain.toml` pins it, `rustup` installs it.
- `protoc` for host builds only (`apt install protobuf-compiler` / `brew
  install protobuf`) — the Docker image installs it itself.
- Docker, optional.

## Quick start

```sh
cargo run --                            # defaults: --pair ethbtc --port 50051
cargo run -- --pair btcusd --port 12345
docker compose up --build               # same thing, containerised
```

Runs four concurrent tasks under one `select!`: the Binance and Bitstamp
feeds (both driven by the same generic `feed::run_feed<E>` loop), the
aggregator, and the gRPC server. Exits the moment any one task ends. Logs go
to stderr; stdout stays empty.

## gRPC server

`src/server.rs` implements `OrderbookAggregator` from `proto/orderbook.proto`
and streams a `Summary` over a `tokio::sync::watch` channel, forwarding a new
value to every subscriber whenever the aggregator publishes one.
`tonic-reflection` is registered, so a client needs no local `.proto` copy:

```sh
grpcurl -plaintext localhost:50051 list
grpcurl -plaintext -max-time 3 localhost:50051 orderbook.OrderbookAggregator/BookSummary
```

Output today is real ETHBTC data but single-venue (`exchange` always reads
`"binance"`) — step 5 makes it a real two-venue merge. Reflection is
unconditionally enabled — convenient for a reviewer, not something
production should ship without a toggle.

## Design decisions

- **`watch`, not `broadcast`.** A book summary is current market state, not
  an event log — a slow subscriber should see the latest snapshot on waking,
  not drain a backlog of stale ones.
- **Merge will walk both venues' already-sorted sides, not sort-the-concat**
  (step 5, not shipped yet). Each exchange already hands over a sorted
  snapshot; re-sorting a combined list throws that information away for no
  benefit at this scale.
- **`Price`/`Amount` are `f64` newtypes, not fixed-point.** Measured first —
  two million ETHBTC price-pair samples showed no disagreement with an `f64`
  at 8 decimals — and dropped as unneeded complexity
  (`specs/002-binance-feed/revisions.md` entry 1). Kept: the newtype
  boundary and a total order (`Ord` via `total_cmp`). Dropped: the integer
  domain.
- **One `Exchange` trait, kept synchronous.** Introduced in step 4 once
  there were two implementations. Synchronous because `async fn` in a trait
  can't be used behind `dyn Trait`, and every call site is a concrete
  generic `E: Exchange` — the trait covers data differences, not control
  flow.
- **Tie-break (step 5's stated intent, not shipped yet):** on an equal
  price, prefer the larger amount, same rule both sides.
- **One process per pair** — `BookSummary` takes `Empty`. See "production."

## Layout

```
proto/orderbook.proto     the gRPC schema, copied verbatim from the brief
build.rs                  compiles the proto via tonic-prost-build
src/config.rs             configuration, read from the environment
src/telemetry.rs          tracing setup (logs to stderr)
src/model.rs              Price/Amount newtypes and Book
src/proxy.rs              HTTP_PROXY/HTTPS_PROXY CONNECT tunnel
src/exchange/{mod,binance,bitstamp}.rs   the Exchange trait, Venue, two impls
src/feed.rs               the one generic run_feed<E: Exchange> driver loop
src/merge.rs              pure book summarisation/merge logic
src/aggregator.rs         owns per-venue state, drives merge, publishes watch
src/server.rs             the OrderbookAggregator gRPC service
src/main.rs               CLI entry point, spawns the four tasks
tests/{cli,grpc}.rs       integration tests: real binary, real tonic client
```

Each file's own doc comment carries the detail this tree doesn't. The
library/binary split is intentional: `tests/` can only import a library
crate, so logic in `main.rs` wouldn't be reachable from it.

## Configuration

Every setting works from a CLI flag or a `KEYROCK_`-prefixed env var, both
with defaults — the binary runs with no flags and an empty environment.
Copy `.env.example` to `.env` to set them without exporting by hand.

| Setting | Flag | Env var | Default |
| --- | --- | --- | --- |
| Pair | `--pair` | `KEYROCK_PAIR` | `ethbtc` |
| Port | `--port` | `KEYROCK_PORT` | `50051` |
| Log level | — | `KEYROCK_LOG_LEVEL` | `info` (`RUST_LOG` wins if set) |
| Host | — | `KEYROCK_HOST` | `127.0.0.1` (use `0.0.0.0` in a container) |

A CLI flag overrides its matching env var. An unparseable `KEYROCK_PORT` is a
startup error, not a silent fallback.

If `HTTPS_PROXY`/`HTTP_PROXY` is set, both feeds tunnel through it via an
HTTP `CONNECT` handshake — `compose.yml` builds this from `PROXY_HOST`/
`PROXY_PORT` in `.env`. Unset, behavior is unchanged.

## Development

```sh
cargo test                                  # unit + integration
cargo clippy --all-targets -- -D warnings   # lints are errors
cargo fmt --check
```

## Docker

```sh
docker compose up --build               # stays running, serves gRPC on 127.0.0.1:50051
docker compose run --rm app --pair btcusd --port 12345
```

Two-stage build (`rust:1.97-slim-bookworm` → `debian:bookworm-slim`
non-root), no `ca-certificates` needed (`rustls-tls-webpki-roots` bundles its
own roots). Binds `0.0.0.0` internally, publishes `127.0.0.1:50051`,
loopback only. **If the container can't reach Binance and no proxy is
configured, it exits shortly after starting** — `select!`'s supervision in
`main.rs` working as designed: a dead feed task ends the whole process
rather than leaving a gRPC server serving stale data.

Development here runs through a CONNECT proxy (Binance is unreachable from
the author's network) — set `PROXY_HOST`/`PROXY_PORT` in `.env` if yours
needs one too.

## What would change for production

**Pair selection belongs on the request, not the process.** `BookSummary`
takes `Empty`, so a second pair means a second process — doesn't scale by
multiplexing (Binance alone allows up to 1024 streams per connection). A
production schema would put the pair on the request. Noted here rather than
edited into `proto/orderbook.proto`, since the given schema is fixed per the
brief.

**Reflection would be gated** behind a config toggle, and the container
would carry a real health check rather than `select!`'s exit-on-failure as
the only signal.
