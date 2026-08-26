# keyrock-case-study

A take-home case study for a Rust engineer application at Keyrock. The
finished service connects to Binance and Bitstamp order-book websocket
feeds, merges the two books for one traded pair, and streams the spread plus
the top 10 bids/asks over gRPC (`proto/orderbook.proto`).

**Steps 0-9 of an 11-step build order have landed.** Both feeds carry real
market data into one aggregator task, which drives a real two-venue
`merge()` and publishes into a `watch` channel that `src/server.rs` streams
to gRPC clients on every change. **The gRPC output is a genuine combined
book** — top 10 bids and asks across both venues, sorted and tie-broken.
A terminal client (`cargo run --bin client`) renders that stream live with
per-venue status (see [Client](#client)). Both feeds survive a disconnect
indefinitely — backoff/jitter, a per-venue staleness filter, and a
grace-period exit if neither venue ever produces data (see
[Reconnection](#reconnection), [Staleness](#staleness)). Ingest-to-publish
latency, the top-ten dedup rate, and a subscriber load curve are all
measured, not argued (see [Measurement](#measurement)).

## Build order

| Step | What it is | Status |
| --- | --- | --- |
| 0 | Scaffold: dependencies, proto, `build.rs`, CLI, config, Docker | Done |
| 1 | Binance feed: connect, parse, print | Done |
| 2 | gRPC server streaming a static/fake `Summary` | Done |
| 3 | Wire step 1 into step 2 — first real end-to-end milestone | Done |
| 4 | Add the Bitstamp feed | Done |
| 5 | Real `merge()` + top-10 + spread — the core deliverable | Done |
| 6 | The example client (`src/bin/client.rs`) | Done |
| 7 | Reconnection + staleness handling | Done |
| 8 | Composition/wiring tests (`specs/010-test-gaps`) | Done |
| 9 | Latency measurement (p50/p99), written up here | Done |
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
docker compose up --build               # server, containerised, zero setup
docker compose run --rm client          # watch the merged book (see Client)
cargo run --bin keyrock-case-study --   # defaults: --pair ethbtc --port 50051
cargo run --bin keyrock-case-study -- --pair btcusd --port 12345
```

`--bin` is required now that this crate builds three binaries — the server,
`src/bin/client.rs`, and `src/bin/loadtest.rs` (see [Client](#client),
[Measurement](#measurement)).

Runs four tasks under a `JoinSet`: both feeds (one generic `feed::run_feed<E>`
loop drives both), the aggregator, and the gRPC server. A feed reconnects on
its own (see [Reconnection](#reconnection)) rather than ending the process;
the aggregator or server ending still does. Logs go to stderr; stdout stays
empty.

## gRPC server

`src/server.rs` implements `OrderbookAggregator` from `proto/orderbook.proto`
and streams a `Summary` over a `tokio::sync::watch` channel, forwarding a new
value to every subscriber whenever the aggregator publishes one.
`tonic-reflection` is registered, so a client needs no local `.proto` copy:

```sh
grpcurl -plaintext localhost:50051 list
grpcurl -plaintext -max-time 3 localhost:50051 orderbook.OrderbookAggregator/BookSummary
```

Output is a real, merged ETHBTC book — `"binance"` and `"bitstamp"` levels
interleaved by price in the same response. Reflection is always on —
convenient here, not production-ready without a toggle.

## Client

`src/bin/client.rs` is a demonstration terminal viewer, not part of the
service — streams `BookSummary` and redraws the combined book in place
(colourised when stdout is a terminal).

```sh
docker compose up -d              # server
docker compose run --rm client    # the book
```

`docker compose run` attaches stdin/stdout to just that one service — no
line prefix, no interleaving with `app`'s logs, which cursor-addressed
redraw needs (`up` would multiplex both services' output and tear it
apart — why `client` sits behind `profiles: ["demo"]`). Without Docker:
`cargo run --bin client -- --addr http://127.0.0.1:50051`.

The header shows each venue's status: `binance ●` if in the most recent
frame, `binance ○ stale 4.2s` if not — inferred client-side from presence in
the streamed levels (`Summary` has no health field), **not** the server's
staleness state ([below](#staleness)): a different clock on a different
event, so it runs a bit behind. Blind spot: a venue publishing normally but
never making the top 10 looks identically stale.

## Reconnection

Both feeds loop inside `src/feed.rs` instead of returning on a closed
socket — a disconnect is a certainty (Binance force-closes at 24h, Bitstamp
can request one). Backoff: 1s/2s/4s/8s/16s, capped 30s, jittered
`0.5x`-`1.5x` per wait, so a shared outage doesn't make every client retry
in the same second and self-inflict a rate-limit hit.

The backoff resets only once a connection has been **stable for 30s**, not
the moment it connects — resetting on connect alone is the trap: a
connection accepted and immediately dropped would reset every cycle,
settling into one attempt per second forever, exactly Binance's
300-per-5-minutes limit approached by a backoff that never actually
engages.

A per-venue token bucket composes with backoff as an absolute ceiling
(backoff: *when*; bucket: *allowed at all*). Binance: capacity 5, refill 1
token/s, from its documented 300-per-5-min limit. Bitstamp publishes no
limit; capacity 5, refill 0.5/s is a stated guess, not fact. Invisible in
normal operation — ~14 attempts per 5 minutes against Binance's 300.

## Staleness

A venue mid-reconnect still has its last book in the aggregator; merging it
in would publish stale prices as live. Before every merge,
`src/aggregator.rs` excludes any venue past its threshold. Binance: 1.5s —
it pushes a snapshot every ~100ms regardless of change, so silence itself
means dead. Bitstamp: **8s, measured live** (ETHBTC, ~5.25 min: 792
messages, max gap 1.795s, median 0.213s) — it only publishes on change, so
8s (~4x the observed max, rounded up for a short sample) avoids reading a
quiet market as dead. If every venue is stale, nothing publishes;
`merge()` itself never sees a clock — staleness is a pre-filter on its
input, not logic inside it.

**Grace period.** Bitstamp accepts any channel name unvalidated, so
`--pair xyzabc` silently produces two connected feeds that never publish,
with nothing distinguishing a bad pair from a quiet market. If no venue has
ever produced data 60s after start, the process exits naming the pair —
60s covers one full backoff cycle (`1+2+4+8+16+30=61s`), so a venue
genuinely mid-reconnect isn't killed before a fair attempt.

## Measurement

Every latency/throughput claim elsewhere in this README was, until this
step, a design argument, not a number. `src/aggregator.rs` now records
three `hdrhistogram` spans around `merge::merge()`, logged every 30s; the
numbers below are from real Binance+Bitstamp connections, not a synthetic
benchmark.

**Prediction, written down before any instrumentation existed:** p50
5-25µs, parse dominant — reasoning was that parsing walks ~2KB of JSON and
~80 float literals per message, while merge is ~20 `f64` comparisons.

**Result: parse was the larger share, but the magnitude was wrong by well
over an order of magnitude.** Debug build: total p50 ~510µs, parse ~55% of
it. Release build: total p50 76-85µs (profile numbers below), parse ~48-51%
of it. Neither span is allocation-bound as predicted — both are dominated by
`tokio` task-wakeup and `mpsc` channel-handoff latency, which
allocation-counting never accounted for. **Merge itself doesn't fill the
merge+publish span** — that span also covers a book queueing behind another
message and the staleness filter, not just `merge()`'s comparisons; the
scheduling/queueing overhead is what actually fills most of it.

**Ingest-to-publish is not wire latency.** Binance's `depth20` stream
carries no event time; only Bitstamp's `microtimestamp` could measure
exchange-to-us delay, and with no Binance counterpart, a cross-venue
comparison on this metric would be misleading — none is reported.

**Duplicate rate: 35-49% across five separate live windows** — clear of the
~30% bar set for building the skip, so it's built: a merge is compared
against the last *published* `Summary` (not `lastUpdateId` — a 15th-level
change doesn't move the top ten) and an identical one isn't sent. Verified
live: the published-update rate dropped from ~4-6/s to ~2.4-2.5/s once the
skip landed.

**Release profile — `lto = "fat"`, `codegen-units = 1`:** build time
36.29s → 53.46s (+47%), live p50 85µs → 76µs (-11%), both from real 5-minute
windows. `panic = "abort"` wasn't added — it would remove
`JoinError::is_panic()`'s panic-vs-cancellation distinction the `JoinSet`
supervisor depends on.

**Load test (`cargo run --bin loadtest`)**, 60s each against the shipped
Docker image, `docker stats` sampled every 5s:

| Subscribers | Aggregate rate | CPU (avg, range) |
| --- | --- | --- |
| 100 | 298 msg/s | 1.5% (0.1-5.4%) |
| 500 | 843 msg/s | 3.4% (0.1-7.5%) |
| 1000 | 2519 msg/s | 11.1% (0.5-24.6%) |

CPU stays low and roughly tracks subscriber count — order-of-magnitude
consistent with the low-thousands-of-subscribers estimate this replaces,
though noisy sample to sample on a shared VM, reported as observed rather
than smoothed into a clean line. **A separate, unplanned finding:** an
instantaneous burst of simultaneous connects (unstaggered) reset most
connections at 500 clients while CPU stayed near-idle — a connect-path
stampede, not the sustained load this measurement is after. Staggering
connects 5ms apart removed the failures entirely; the table above is from
the staggered runs.

**24-hour run: started 2026-08-25T22:58:10Z**, commit `02754a4` (`docker
compose up -d --build app`), so it exercises the shipped build rather than
an intermediate one. Binance force-closes every connection at 24h,
documented — this is the only thing that actually exercises the
reconnection path against the condition it was built for; a proxy
interruption lasting seconds, used everywhere else in this project's
testing, doesn't. **Pending** — reconnect counts per venue, whether
Binance's scheduled close shows up as one of them, p50/p99 drift from
start to end, peak RSS, staleness-exclusion counts, and the full-run
duplicate rate land here once the run completes; not reported before it
does.

## Design decisions

- **`watch`, not `broadcast`.** A book summary is current market state, not
  an event log — see [Behaviour under load](#what-id-change-for-production).
- **`merge()` walks both venues' already-sorted sides with peekable
  cursors, not sort-the-concat** — "don't discard information you're
  handed," not a speed claim.
- **`Price`/`Amount` are `f64` newtypes, not fixed-point** — measured first,
  two million ETHBTC price-pair samples showed no disagreement with `f64`
  at 8 decimals (`specs/002-binance-feed/revisions.md` entry 1). Kept: the
  newtype boundary and a total order (`Ord` via `total_cmp`). The spread,
  the one computed value, rounds to that same 8-decimal tick at the boundary.
- **One `Exchange` trait, kept synchronous** — `async fn` in a trait can't
  be used behind `dyn Trait` anyway, and every call site is a concrete
  generic `E: Exchange`.
- **Tie-break: equal price prefers the larger amount, same rule both
  sides** — a `BTreeMap<Venue, _>` (not `HashMap`) makes a full tie resolve
  deterministically to whichever venue sorts first (Binance).
- **A crossed book publishes a negative spread on purpose**, not a bug —
  two independently-matched venues routinely cross even though one
  exchange's own matching engine can't; a real, if fleeting, arbitrage
  signal.
- **One process per pair** — `BookSummary` takes `Empty`, so a second pair
  means a second process.

## Layout

```
.
├── proto/orderbook.proto   gRPC schema
├── build.rs                compiles the proto
├── src/
│   ├── config.rs           env config
│   ├── telemetry.rs        tracing, stderr
│   ├── model.rs            Price/Amount, Book
│   ├── proxy.rs            CONNECT tunnel
│   ├── exchange/
│   │   ├── mod.rs          Exchange trait, Venue
│   │   ├── binance.rs      Binance impl
│   │   └── bitstamp.rs     Bitstamp impl
│   ├── feed.rs             generic run_feed<E>
│   ├── merge.rs            pure merge
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

Each file's own doc comment carries the detail this tree doesn't. The
library/binary split is intentional: `tests/` can only import a library
crate, so `main.rs` logic isn't reachable from it.

## Configuration

Every setting works from a CLI flag or a `KEYROCK_`-prefixed env var, both
with defaults — copy `.env.example` to `.env` to set them without exporting
by hand. A flag overrides its env var; an unparseable `KEYROCK_PORT` is a
startup error.

| Setting | Flag | Env var | Default |
| --- | --- | --- | --- |
| Pair | `--pair` | `KEYROCK_PAIR` | `ethbtc` |
| Port | `--port` | `KEYROCK_PORT` | `50051` |
| Log level | — | `KEYROCK_LOG_LEVEL` | `info` (`RUST_LOG` wins if set) |
| Host | — | `KEYROCK_HOST` | `127.0.0.1` (use `0.0.0.0` in a container) |

If `HTTPS_PROXY`/`HTTP_PROXY` is set, both feeds tunnel through it via HTTP
`CONNECT` (`compose.yml` builds this from `PROXY_HOST`/`PROXY_PORT` in
`.env`); unset, behavior is unchanged.

## Development

```sh
cargo test                                  # unit + integration
cargo clippy --all-targets -- -D warnings   # lints are errors
cargo fmt --check
```

## Docker

```sh
docker compose up --build               # app, gRPC on 127.0.0.1:50051
docker compose run --rm app --pair btcusd --port 12345
docker compose run --rm client          # the demo viewer — see Client above
```

Two services, one image: `app` (the server) and `client` (the demo viewer,
sharing `app`'s image via `image:` rather than rebuilding). `client` sits
behind `profiles: ["demo"]`, so `docker compose up` starts only `app`. See
[Client](#client) for why.

Two-stage build (`rust:1.97-slim-bookworm` → `debian:bookworm-slim`
non-root), no `ca-certificates` needed (`rustls-tls-webpki-roots` bundles its
own roots). Binds `0.0.0.0` internally, publishes `127.0.0.1:50051`,
loopback only. **The container doesn't exit just because a venue is
unreachable** — it reconnects with backoff (see [Reconnection](#reconnection)),
only exiting if neither venue has produced data after 60s (see
[Staleness](#staleness)). Development here runs through a CONNECT proxy
(Binance is unreachable from the author's network); set `PROXY_HOST`/
`PROXY_PORT` in `.env` if yours needs one too.

## What I'd change for production

| Limitation | What I'd do |
| --- | --- |
| Pair is per process — `BookSummary` takes `Empty` | Pair on the request; Binance carries 1024 streams per connection |
| Tick size hardcoded at 8 decimals | Per-pair tick from each venue's `exchangeInfo` |
| `merge()` returns the proto types | An internal type, once there's a second consumer |
| Reflection always on | Behind a config toggle |
| `Level.exchange` allocates per level | Only matters if it shows up in a profile |
| Venue health inferred client-side from which levels show up | Carry it on the wire — a venue publishing but never in the top 10 looks identically stale |
| Parser doesn't validate price signs, reports what the venue sent | A corrupted negative price would sort to book-front and propagate; validation belongs a layer up, isn't built here |
| Every subscriber gets its own clone + protobuf encode of identical bytes | Encode once, hand out `Bytes` — the only per-publish cost that scales with N (below) |

### Behaviour under load

A slow subscriber degrades only itself — `watch` holds only the latest
value, so a caught-up client gets the current book, not a backlog
(`broadcast` would grow memory or drop with `Lagged` instead). Feeds and
the aggregator are unaffected by subscriber count. The one cost that scales
with N is the per-subscriber clone+encode, measured at
[Measurement](#measurement)'s load-test table — encoding once and handing
out `Bytes` (table above) is the fix.

**No connection limit** — fine behind known consumers, not for a public
deployment; `tower`'s concurrency-limit layer is the standard fix, not
built because this isn't that kind of service. **Dedup is built** — the
measured duplicate rate (35-49%, [Measurement](#measurement)) cleared the
bar for building it.

