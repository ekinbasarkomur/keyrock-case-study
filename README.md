# keyrock-case-study

A take-home case study for a Rust engineer role at Keyrock. It connects to
Binance and Bitstamp order-book feeds, merges them into one book, and
streams the spread plus the top 10 bids/asks over gRPC
(`proto/orderbook.proto`).

Steps 0-9 of 11 are done. Both feeds carry real market data. One aggregator
task merges them and publishes on a `watch` channel; the gRPC server
streams that to clients on every change. A feed reconnects on its own. A
stale venue drops out of the merge. A pair that never produces data exits
after 60s. A terminal client shows the live book. Real latency and
throughput numbers are in [Measurement](#measurement).

## Build order

| Step | What | Status |
| --- | --- | --- |
| 0 | Scaffold: deps, proto, build.rs, CLI, config, Docker | Done |
| 1 | Binance feed | Done |
| 2 | gRPC server, fake data | Done |
| 3 | Real data end to end | Done |
| 4 | Bitstamp feed | Done |
| 5 | Real merge, top 10, spread | Done |
| 6 | Demo client | Done |
| 7 | Reconnection, staleness | Done |
| 8 | Composition/wiring tests | Done |
| 9 | Latency measurement | Done |
| 10 | Final cleanup | Not started |

## Requirements

- Rust 1.97.1, pinned in `rust-toolchain.toml`.
- `protoc` for host builds (`brew install protobuf` / `apt install
  protobuf-compiler`) — the Docker image installs it itself.
- Docker, optional.

## Quick start

```sh
docker compose up --build
docker compose run --rm client
cargo run --bin keyrock-case-study
cargo run --bin keyrock-case-study -- --pair btcusd --port 12345
```

Three binaries now, so `--bin` picks one: the server, `client`, and
`loadtest` (see [Measurement](#measurement)).

Four tasks run together: both feeds, the aggregator, the gRPC server. A
feed dying reconnects on its own; the aggregator or server dying ends the
process. Logs go to stderr, stdout stays empty.

## gRPC server

`src/server.rs` implements `OrderbookAggregator` and streams a `Summary`
whenever the book changes. Reflection is on, so no local `.proto` is
needed:

```sh
grpcurl -plaintext localhost:50051 list
grpcurl -plaintext -max-time 3 localhost:50051 orderbook.OrderbookAggregator/BookSummary
```

The output is a real merged book — both venues, sorted by price.

## Client

`src/bin/client.rs` is a demo terminal viewer, not part of the service.

```sh
docker compose up -d
docker compose run --rm client
```

Use `run`, not `up` — `up` interleaves both services' logs and breaks the
in-place redraw. Without Docker: `cargo run --bin client -- --addr
http://127.0.0.1:50051`.

The header shows each venue's status (`binance ●`, or `binance ○ stale
4.2s`), guessed client-side from what's actually in the stream. That's not
the server's own staleness check, so it lags a bit behind it.

## Reconnection

A feed never gives up on a disconnect — it's a certainty, not an edge case
(Binance force-closes every connection at 24h). Backoff: 1s, 2s, 4s, 8s,
16s, capped at 30s, jittered so a shared outage doesn't make every client
retry in the same second.

The backoff only resets once a connection has stayed up for 30s. Resetting
on connect alone would let a connect-then-drop loop settle into one attempt
a second, forever.

A token bucket caps attempts per venue on top of backoff. Binance: 5
capacity, 1/s refill, from its documented limit. Bitstamp publishes no
limit; 5 capacity, 0.5/s is a guess, not a fact.

## Staleness

A venue mid-reconnect still has an old book sitting in the aggregator.
Publishing it as current would be wrong. Before every merge, any venue past
its threshold is dropped. Binance: 1.5s — it pushes a snapshot every 100ms,
so silence means dead. Bitstamp: 8s, measured live (max gap seen: 1.8s over
~5 minutes) — it only publishes on change, so it needs more slack. If every
venue is stale, nothing publishes.

Grace period: Bitstamp accepts any pair name, so a typo silently produces
two feeds that connect and never publish. If no venue has produced data
after 60s, the process exits and names the pair.

## Measurement

Every number below is from a real run against Binance and Bitstamp, not a
synthetic benchmark. `src/aggregator.rs` times each book with
`hdrhistogram` and logs p50/p99/p99.9 every 30s.

**Prediction, written down before any code:** p50 5-25µs, parse the biggest
cost.

**Result:** parse was the bigger share, but the whole prediction was off by
10-50x. Debug build: p50 ~510µs. Release build: p50 76-85µs. Parse is
~50% of that either way. Neither number comes from allocations or
comparisons — both spans are dominated by `tokio` and channel-handoff
scheduling, which the prediction never accounted for. `merge()` itself is
not the slow part.

Ingest-to-publish is not wire latency. Binance's stream carries no
timestamp, so there's nothing to compare it against on that side.

**Duplicate rate: 35-49%**, measured across five separate runs — clear of
the 30% bar set for building dedup, so it's built: a merge that matches the
last published `Summary` isn't sent again.

**Release profile** (`lto = "fat"`, `codegen-units = 1`): build time 36s →
53s, live p50 85µs → 76µs. `panic = "abort"` was left out — it would break
how the process tells a task panic from a cancellation.

**Load test** (`cargo run --bin loadtest`), 100/500/1000 subscribers, 60s
each, against the real Docker image:

| Subscribers | Rate | CPU (avg) |
| --- | --- | --- |
| 100 | 298 msg/s | 1.5% |
| 500 | 843 msg/s | 3.4% |
| 1000 | 2519 msg/s | 11.1% |

CPU stays low and roughly tracks subscriber count. One catch found along
the way: connecting all clients at once reset most of them, while CPU sat
idle — a connection burst, not a load problem. Spacing the connects out by
5ms each fixed it; the table above is from the fixed version.

**24-hour run: started 2026-08-25T22:58:10Z, commit `02754a4`.** Binance
closes every connection at 24h, so this is the only real test of the
reconnection path against what it was built for. **Pending** — reconnect
counts, latency drift, peak memory, and the full-run duplicate rate land
here once it finishes.

## Design decisions

- **`watch`, not `broadcast`.** A book summary is current state, not an
  event log.
- **`merge()` walks both venues' already-sorted lists instead of sorting
  the concat.** Don't throw away work the exchange already did.
- **`Price`/`Amount` are `f64`, not fixed-point.** Checked first: two
  million real ETHBTC price pairs never disagreed with fixed-point at 8
  decimals (`specs/002-binance-feed/revisions.md`).
- **One `Exchange` trait, kept synchronous.** `async fn` in a trait can't
  be used behind `dyn Trait` anyway, and every call site is a concrete
  type.
- **Tie-break: same price, bigger amount wins, both sides.** A full tie
  (same price and amount) goes to whichever venue was declared first
  (Binance).
- **A crossed book publishes a negative spread on purpose.** Two venues
  with no shared matching engine can cross for real; that's a signal, not
  a bug.
- **One process per pair** — `BookSummary` takes no arguments, so a second
  pair means a second process.

## Layout

```
.
├── proto/orderbook.proto   gRPC schema
├── build.rs                compiles the proto
├── src/
│   ├── config.rs           env config
│   ├── telemetry.rs        logging, stderr
│   ├── model.rs            Price/Amount, Book
│   ├── proxy.rs            HTTP CONNECT tunnel
│   ├── exchange/
│   │   ├── mod.rs          Exchange trait, Venue
│   │   ├── binance.rs      Binance impl
│   │   └── bitstamp.rs     Bitstamp impl
│   ├── feed.rs             shared reconnect loop
│   ├── merge.rs            the pure merge
│   ├── aggregator.rs       state, publishes watch
│   ├── server.rs           gRPC service
│   ├── main.rs             CLI entry
│   └── bin/
│       ├── client.rs       demo viewer
│       └── loadtest.rs     load harness
└── tests/
    ├── cli.rs              real binary
    ├── grpc.rs             real tonic client
    └── feed.rs             run_feed, local socket
```

Each file's own doc comment has the detail this tree skips. `tests/` only
imports the library, so nothing in `main.rs` is reachable from there —
that's why it stays thin.

## Configuration

Every setting is a CLI flag or a `KEYROCK_`-prefixed env var, both with
defaults. A flag wins over its env var. Copy `.env.example` to `.env` to
set them without exporting by hand.

| Setting | Flag | Env var | Default |
| --- | --- | --- | --- |
| Pair | `--pair` | `KEYROCK_PAIR` | `ethbtc` |
| Port | `--port` | `KEYROCK_PORT` | `50051` |
| Log level | — | `KEYROCK_LOG_LEVEL` | `info` (`RUST_LOG` wins if set) |
| Host | — | `KEYROCK_HOST` | `127.0.0.1` (use `0.0.0.0` in a container) |

If `HTTPS_PROXY`/`HTTP_PROXY` is set, both feeds tunnel through it.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Docker

```sh
docker compose up --build
docker compose run --rm app --pair btcusd --port 12345
docker compose run --rm client
```

One image, two services. `client` sits behind a `demo` profile, so `up`
only starts `app`.

Two-stage build, no `ca-certificates` needed. Binds `0.0.0.0` inside,
publishes `127.0.0.1:50051` on the host. A dead venue doesn't kill the
container — it reconnects. It only exits if neither venue ever produces
data.

## What I'd change for production

| Limitation | Fix |
| --- | --- |
| One pair per process | Pair on the request |
| Tick size hardcoded at 8 decimals | Per-pair tick from each venue |
| `merge()` returns proto types directly | An internal type, once there's a second consumer |
| Reflection always on | A config toggle |
| No cap on concurrent gRPC connections | `tower`'s concurrency-limit layer |
| Client guesses venue health from the stream | Put it on the wire |
| Parser doesn't check price sign | A bad frame would sort to the front and go out unfiltered |
| Every subscriber gets its own clone + encode | Encode once, hand out bytes — see [Measurement](#measurement) for the real cost |

A slow subscriber only hurts itself — `watch` holds the latest value, not a
queue, so a caught-up client sees the current book, not a backlog. Feeds
and the aggregator don't slow down as subscribers grow. Dedup is built
(see Measurement). No connection limit is built.
