# keyrock-case-study

A take-home case study for a Rust engineer role at Keyrock. It connects to
Binance, Bitstamp, and Kraken order-book feeds, merges them into one book,
and streams the spread plus the top 10 bids/asks over gRPC
(`proto/orderbook.proto`).

Steps 0-9 of 11 are done. All three feeds carry real market data. One
aggregator task merges them and publishes on a `watch` channel; the gRPC
server streams that to clients on every change. A feed reconnects on its
own. A stale venue drops out of the merge. A pair that never produces data
exits after 60s. A terminal client shows the live book. Real latency and
throughput numbers are in [Measurement](#measurement). Kraken was added
after the brief's two required venues, as a check on how cheap a third
venue really is — see [Kraken](#kraken) for what that cost.

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

Five tasks run together: all three feeds, the aggregator, the gRPC server.
A feed dying reconnects on its own; the aggregator or server dying ends the
process. Logs go to stderr, stdout stays empty.

## gRPC server

`src/server.rs` implements `OrderbookAggregator` and streams a `Summary`
whenever the book changes. Reflection is on, so no local `.proto` is
needed:

```sh
grpcurl -plaintext localhost:50051 list
grpcurl -plaintext -max-time 3 localhost:50051 orderbook.OrderbookAggregator/BookSummary
```

The output is a real merged book — all three venues, sorted by price.

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
capacity, 1/s refill, from its documented limit. Bitstamp and Kraken
publish no limit; 5 capacity, 0.5/s is a guess, not a fact, for both.

## Staleness

A venue mid-reconnect still has an old book sitting in the aggregator.
Publishing it as current would be wrong. Before every merge, any venue past
its threshold is dropped. Binance: 1.5s — it pushes a snapshot every 100ms,
so silence means dead. Bitstamp: 8s, measured live (max gap seen: 1.8s over
~5 minutes). Kraken: 12s, measured live the same way (max gap seen: 2.9s
over 300s, 16,444 book messages) — like Bitstamp, it only publishes on
change, so it needs more slack. If every venue is stale, nothing publishes.

Grace period: Bitstamp accepts any pair name, so a typo silently produces
two feeds that connect and never publish. If no venue has produced data
after 60s, the process exits and names the pair.

## Kraken

Added after the brief's two required venues, to find out what a third one
actually costs given the architecture was supposedly built for it. Two of
`src/exchange/mod.rs`, `src/feed.rs`, `src/main.rs` picked up small,
mechanical additions (a `Venue::Kraken` arm, a scoped ping branch, a third
spawn); `src/exchange/kraken.rs` did not turn out to be mechanical.

**Kraken's book channel isn't self-contained per message, unlike
Binance/Bitstamp's.** It sends one full snapshot on subscribe, then only
the changed levels after that (`qty: 0` means remove). Binance's
`depth20@100ms` and this project's chosen Bitstamp channel both resend the
full top-N every message, which is why `Exchange::parse(&self, ...)` could
stay a pure function of one message. Kraken can't work that way — producing
a real book means holding the last one and patching it. `Kraken` carries
that state in a `std::sync::Mutex<Option<Book>>` (`RefCell` was tried
first and compiled and passed its own tests in isolation, but failed a
real crate-wide build: `RefCell` is `!Sync`, and `tokio::spawn` needs the
holding future to be `Send` — a gap an isolated module test can't see).
Nothing about this is actually concurrent — one connection owns the
`Mutex` at a time — it exists only to satisfy that bound.

The consequence worth naming: `Kraken::parse` is order-dependent.
Binance/Bitstamp's `parse` is safe to call twice with the same input;
Kraken's silently double-applies a delta if it's ever fed the same update
twice. Documented loudly on the type, not left implicit.

Two more things it needed that the other two didn't: a symbol converter
(Kraken wants `"ETH/BTC"`, this project's `--pair` is `"ethbtc"`), and a
CRC32 checksum — Kraken sends one with every message; verified here,
confirmed bit-exact against a real captured snapshot before trusting it,
because getting it wrong is easy (reformatting a parsed `f64` back to text
drops trailing zeros and silently breaks it — the checksum has to run
against the original wire digits, not a round-tripped float).

## Measurement

Every number below is from a real run, not a synthetic benchmark.
`src/aggregator.rs` times each book with `hdrhistogram` and logs
p50/p99/p99.9 every 30s. Each histogram resets right after it's read, so
every log line describes the last ~30s, not the process's whole lifetime —
a bad tick shows up in one report and is gone from the next, not
permanently stuck in p999. The tradeoff is the flip side of that: a true
once-ever worst case is only visible in the single window it happened in,
never again — this trades "worst ever" for "worst recently" on purpose.

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

**24-hour run: started 2026-08-26T13:09:46Z, commit `02754a4`, ran 32+
hours.** Zero reconnects, zero errors — but that also means **Binance's
documented 24h forced close never actually triggered in this run**; that
specific claim stays unconfirmed here, not verified, and isn't restated as
fact elsewhere in this README. p50 drifted from ~60µs to ~94µs over the
run — real, not noise, cause not diagnosed (could be the accumulating
histogram above, could be a genuine change in book activity). Duplicate
rate drifted from ~15% to ~33% over the same window, likely market
activity rather than anything in the code. This run is being superseded by
a fresh one once the histogram windowing gap above is fixed, so these
numbers won't be updated further — they're reported as what was actually
observed, not smoothed into "stable" because that's what was expected.

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
  type. Kraken's `parse` needs interior-mutable state (see
  [Kraken](#kraken)) — the trait signature itself didn't change to
  accommodate that, only Kraken's own implementation did.
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
│   │   ├── bitstamp.rs     Bitstamp impl
│   │   └── kraken.rs       Kraken impl (see Kraken)
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
