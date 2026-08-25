# keyrock-case-study

A take-home case study for a Rust engineer application at Keyrock. The
finished service connects to Binance and Bitstamp order-book websocket
feeds, merges the two books for one traded pair, and streams the spread plus
the top 10 bids/asks over gRPC (`proto/orderbook.proto`).

**Steps 0-8 of an 11-step build order have landed.** Both feeds carry real
market data into one aggregator task, which drives a real two-venue
`merge()` and publishes into a `watch` channel that `src/server.rs` streams
to gRPC clients on every change. **The gRPC output is a genuine combined
book** — top 10 bids and asks across both venues, sorted and tie-broken.
A terminal client (`cargo run --bin client`) renders that stream live with
per-venue status (see [Client](#client)). Both feeds survive a disconnect
indefinitely — backoff/jitter, a per-venue staleness filter, and a
grace-period exit if neither venue ever produces data (see
[Reconnection](#reconnection), [Staleness](#staleness)).

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
| 9 | Latency measurement (p50/p99), written up here | Not started |
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

`--bin` is required now that this crate builds two binaries — the server and
`src/bin/client.rs` (see [Client](#client)).

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

Output is a real, merged ETHBTC book — both `"binance"` and `"bitstamp"`
levels appear in the same response, interleaved by price. Reflection is
unconditionally enabled — convenient for a reviewer, not production-ready
without a toggle.

## Client

`src/bin/client.rs` is a demonstration terminal viewer, not part of the
service — a second binary that streams `BookSummary` and redraws the
combined book in place (colourised when stdout is a terminal).

```sh
docker compose up -d              # server
docker compose run --rm client    # the book
```

`docker compose run` attaches stdin/stdout to just that one service — no
line prefix, no interleaving with `app`'s logs, which cursor-addressed
redraw needs; `docker compose up` would multiplex both services' output and
tear the redraw apart (why the service stays out of `up`'s default set,
`profiles: ["demo"]` in `compose.yml`). Locally, without Docker:
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

## Design decisions

- **`watch`, not `broadcast`.** A book summary is current market state, not
  an event log (see [Behaviour under load](#what-id-change-for-production)
  for the fan-out payoff this buys).
- **`merge()` walks both venues' already-sorted sides with peekable
  cursors, not sort-the-concat** — "don't discard information you're
  handed," not a speed claim.
- **`Price`/`Amount` are `f64` newtypes, not fixed-point.** Measured first —
  two million ETHBTC price-pair samples showed no disagreement with an `f64`
  at 8 decimals (`specs/002-binance-feed/revisions.md` entry 1). Kept: the
  newtype boundary and a total order (`Ord` via `total_cmp`). The spread,
  the one computed value, is rounded to that same 8-decimal tick at the
  boundary.
- **One `Exchange` trait, kept synchronous** — `async fn` in a trait can't
  be used behind `dyn Trait` anyway, and every call site is a concrete
  generic `E: Exchange`.
- **Tie-break: equal price prefers the larger amount, same rule both
  sides.** A `BTreeMap<Venue, _>` (not `HashMap`) makes a full price+amount
  tie resolve deterministically to whichever venue sorts first (Binance).
- **A crossed book publishes a negative spread on purpose** — not a bug.
  Two independently-matched venues routinely cross even though one
  exchange's own matching engine can't; a real (if fleeting) arbitrage
  signal worth reporting honestly.
- **One process per pair** — `BookSummary` takes `Empty`, so a second pair
  means a second process.

## Layout

```
.
├── proto/orderbook.proto   gRPC schema, from the brief
├── build.rs                compiles the proto
├── src/
│   ├── config.rs           env-based configuration
│   ├── telemetry.rs        tracing (logs to stderr)
│   ├── model.rs            Price/Amount, Book
│   ├── proxy.rs            HTTP(S)_PROXY CONNECT tunnel
│   ├── exchange/
│   │   ├── mod.rs          Exchange trait, Venue
│   │   ├── binance.rs      Binance impl
│   │   └── bitstamp.rs     Bitstamp impl
│   ├── feed.rs             generic run_feed<E> driver
│   ├── merge.rs            pure merge: Side, merge_side, merge
│   ├── aggregator.rs       per-venue state, publishes watch
│   ├── server.rs           OrderbookAggregator gRPC service
│   ├── main.rs             CLI entry, spawns the tasks
│   └── bin/
│       └── client.rs       demo terminal viewer, second binary
└── tests/
    ├── cli.rs              real binary
    ├── grpc.rs             real tonic client
    └── feed.rs             run_feed against a local socket
```

Each file's own doc comment carries the detail this tree doesn't. The
library/binary split is intentional: `tests/` can only import a library
crate, so `main.rs` logic isn't reachable from it.

## Configuration

Every setting works from a CLI flag or a `KEYROCK_`-prefixed env var, both
with defaults. Copy `.env.example` to `.env` to set them without exporting
by hand.

| Setting | Flag | Env var | Default |
| --- | --- | --- | --- |
| Pair | `--pair` | `KEYROCK_PAIR` | `ethbtc` |
| Port | `--port` | `KEYROCK_PORT` | `50051` |
| Log level | — | `KEYROCK_LOG_LEVEL` | `info` (`RUST_LOG` wins if set) |
| Host | — | `KEYROCK_HOST` | `127.0.0.1` (use `0.0.0.0` in a container) |

A CLI flag overrides its env var; an unparseable `KEYROCK_PORT` is a startup
error. If `HTTPS_PROXY`/`HTTP_PROXY` is set, both feeds tunnel through it
via an HTTP `CONNECT` handshake (`compose.yml` builds this from
`PROXY_HOST`/`PROXY_PORT` in `.env`); unset, behavior is unchanged.

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

Load lands in three places; the design isolates two.

A slow subscriber degrades only itself — `tonic` stops polling its
backed-up stream, `watch` holds only the latest value, so a caught-up
client gets the current book, not a backlog (`broadcast` would instead
grow memory or drop it with `Lagged`). The aggregator and feeds are
unaffected by subscriber count — `watch::send` doesn't block; clone/encode
work happens per subscriber, in that subscriber's own task.

What *does* scale with N is that encode: each subscriber gets its own deep
clone and protobuf encode of identical bytes. Unmeasured, but roughly —
20 publishes/s, microseconds each — saturates a core somewhere in the low
thousands of subscribers. The only cost that grows with N, which is why
encoding once and handing out `Bytes` (table above) is the first thing
worth changing.

**No connection limit.** Nothing caps concurrent gRPC connections — fine
behind known consumers, not for a public deployment, since the encode
cost is linear in N. `tower`'s concurrency-limit layer is the standard
fix; not built because this isn't that kind of service.

**Dedup isn't built yet, deliberately.** Binance publishes every 100ms
regardless of change, so some ticks re-publish an identical top ten for
no reason. Fix is two lines (keep the last `Summary`, skip an unchanged
send) — also closer to the brief's "stream on every *change*" wording.
Held back because how often the top ten repeats isn't measured, and this
project avoids optimizing before measuring; lands with the latency work.
Whoever builds it: compare the published `Summary`, not `lastUpdateId` —
a venue's 15th level can change while the top ten doesn't.

