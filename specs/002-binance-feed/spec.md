---
spec_name: "Step 1 — Binance feed"
spec_id: "002"
spec_folder: "002-binance-feed"
status: "draft"
created_at: "2026-08-23"
updated_at: "2026-08-23"
created_by: "spec-synthesizer"
creation_mode: "human-brief"
source_inputs:
  - "inputs/human.md (kept locally, gitignored — raw briefs aren't published)"
source_agents: []
goal: "Connect to Binance's depth20@100ms websocket, parse each snapshot into an internal fixed-point Book, and print one readable summary line per update — nothing else."
purpose: "Step 1 of the 11-step build order proves the websocket-to-internal-model path end to end, on a single exchange, before the second feed, concurrency, or gRPC exist — so the price-representation decision and the parser's tolerance for non-book messages are both settled and tested while there is still only one exchange to reason about."
parent_request: "step-1 binance-feed brief, 2026-08-23"
related_paths:
  - "src/model.rs"
  - "src/exchange/mod.rs"
  - "src/exchange/binance.rs"
  - "src/main.rs"
  - "src/lib.rs"
  - "README.md"
verification_level: "mixed"
complexity: "small"
---

# Spec: 002-binance-feed

## Problem

Step 0 landed a compiling, containerised scaffold with no business logic: no
websocket client, no internal book model, no exchange parsing. The 11-step
build order's first real behaviour is a single Binance feed — connect, parse,
print — proving the price-representation choice and the "non-book message
tolerance" pattern that every later feed and the merge step depend on. No
code should land until this spec is approved.

## Goal

After this step:

- `src/model.rs` defines `Price` and `Amount` as newtypes over `i64` (fixed
  scale `1e9`), and a `Book` type (bids/asks as ordered `Vec<(Price, Amount)>`
  plus `last_update_id`).
- `src/exchange/binance.rs` connects to
  `wss://stream.binance.com:9443/ws/{pair}@depth20@100ms`, reads frames in a
  loop, and a pure `parse(&str) -> Option<Book>` function turns a text message
  into a `Book` or `None`.
- `src/main.rs` is `async fn main()` (via `#[tokio::main]`) and drives the
  read loop directly — no `tokio::spawn`, no `.split()` on the websocket
  stream.
- Running the binary prints one `tracing` line per update: best bid, best
  ask, `lastUpdateId`, in readable decimal.
- `README.md` gains a "Price representation" section under design decisions.
- The connection is empirically verified to survive Binance's 20s
  ping / 60s pong-timeout over a 25+ minute run.

## Purpose

Generalising an `Exchange` trait from one implementation produces the wrong
abstraction — the trait arrives at step 4, once a second venue exists to show
what actually varies. Likewise, concurrency primitives (`spawn`, channels)
have no job to do with one feed; they arrive at step 3/4. This step is
deliberately narrow so the reviewer can see the price-representation and
parser-tolerance decisions in isolation, without concurrency or gRPC noise
around them.

## Scope

**In:**

- `src/model.rs` — `Price`, `Amount` newtypes over `i64` at scale `1e9`;
  `Book` (exchange-agnostic: bids, asks, `last_update_id`).
- `src/exchange/mod.rs`, `src/exchange/binance.rs` — connect, read loop,
  `parse(&str) -> Option<Book>`. No trait.
- `src/main.rs` — becomes `async`, drives the Binance read loop directly in
  the `main` task.
- Tests for the parser and for the price conversion (see Testing Strategy).
- `README.md` update (Price representation section).

**Out — do not write these, and do not leave stub files or `todo!()` for
them:**

- Any `Exchange` trait or cross-exchange abstraction.
- `tokio::spawn`.
- `.split()` on the websocket stream.
- Reconnection, backoff, or staleness handling.
- Anything gRPC (`src/server.rs`, use of the generated `orderbook` types).
- `merge.rs`, `src/aggregator.rs`.

## Current State

Verified by reading the files directly, not assumed (per step 0's finished
state — `specs/001-step-0-foundation/`):

- `src/lib.rs` declares `pub mod config; pub mod telemetry;` only — no
  `model` or `exchange` module yet.
- `src/main.rs` is synchronous, parses `--pair`/`--port`, builds
  `Config::from_env()`, calls `telemetry::init(...)`, logs one startup line,
  returns `Ok(())`. No websocket code, no async runtime use.
- `Cargo.toml` already carries `tokio` (`full`), `tokio-tungstenite`
  (`rustls-tls-webpki-roots`), `futures-util`, `serde`/`serde_json`,
  `tracing`/`tracing-subscriber` — everything this step needs is already a
  dependency; nothing new to add.
- `Config` already has a `pair` field (default `"ethbtc"`) read from
  `ORDERBOOK_PAIR` / `--pair`, which this step's feed will consume directly —
  lowercased, as the endpoint requires.
- No `src/model.rs` or `src/exchange/` directory exists yet.

## Proposed Design

### `src/model.rs`

`Price` and `Amount`: newtypes over `i64`, fixed scale `1_000_000_000` (1e9).
Parsing an exchange string (`"0.03150000"`) multiplies by the scale and
rounds to the nearest integer tick, immediately on receipt — no float travels
past this conversion. `Display` renders the decimal form at the scale's
precision (`0.03150000`); `Debug` shows the raw `i64`. The two types are
distinct so the compiler rejects passing an amount where a price is expected.

`Book`: bids and asks as ordered `Vec<(Price, Amount)>` (already
best-first, per Binance's own sorted snapshot), plus `last_update_id: u64`.
Exchange-agnostic — nothing Binance-specific lives here.

**Why not `f64` — the argument as stated, not overstated.** The comment in
`model.rs` (and the mirrored README section) must make the modelling
argument first: a price is not a continuous quantity, it's an integer
multiple of a tick, and representing a discrete quantity with a continuous
type is a category error before it's a precision one. The measured numbers
are the second point, stated as an honest bound on how small the precision
benefit actually is here — subtracting `0.031505 - 0.031500` in `f64` yields
a relative error of `~3.9e-13`, invisible at 8 decimals — not as the
justification (a claim of "`f64` can't represent decimals, so the spread
would be wrong" does not survive the follow-up "how wrong?"). Third: where
the small numeric error would stop being small — arithmetic that
accumulates, e.g. summing many levels or compounding positions — named
explicitly as the case this step does not have, since it performs one
subtraction.

**Scale decision and its cost.** `i64` at scale `1e9` is a documented
assumption, not derived from exchange metadata (that's the step 4/production
answer, via Binance's `exchangeInfo`, and is out of scope here). At this
scale: the largest representable price is `i64::MAX / 1e9 ≈ 9.22 × 10^9` in
whatever unit the pair quotes (far beyond any realistic price for the
configured pair); the smallest representable tick is `1 / 1e9 = 1e-9` in that
same unit — finer than any real exchange tick size, so no precision is lost
at the low end either. Both bounds are stated in the README rather than left
implicit.

### `src/exchange/mod.rs`, `src/exchange/binance.rs`

No trait — a single concrete module. `binance.rs` owns:

- The connect URL: `wss://stream.binance.com:9443/ws/{pair}@depth20@100ms`,
  pair lowercased.
- The read loop: iterate the websocket stream directly in `main`'s async
  context (no `spawn`, no `.split()` — a single `StreamExt::next()` loop over
  the already-bidirectional `WebSocketStream` is sufficient since this step
  never needs to write anything itself beyond what `tokio-tungstenite`
  answers automatically).
- `parse(text: &str) -> Option<Book>`: deserializes the Binance payload
  (`lastUpdateId`, `bids`, `asks` as `Vec<[String; 2]>`) via `serde_json`,
  converts each price/amount string through `model::Price`/`Amount`'s parser,
  and returns `Some(Book)`. Any message that isn't a recognizable book
  payload (`{"e":"serverShutdown",...}`, malformed JSON, anything without the
  expected fields) returns `None` — logged at `debug`, not an error.

**Message-variant handling is explicit, not caught by a wildcard:**

- `Message::Text(t)` → the real work, passed to `parse`.
- `Message::Ping(_)` / `Message::Pong(_)` → ignored; `tokio-tungstenite`
  answers pings automatically.
- `Message::Close(_)` → breaks the read loop (this step does not reconnect —
  that's step 6).
- `Message::Binary(_)` → logged at `debug` and skipped.

`parse` returning `Result` and using `?` was explicitly rejected: a
`serverShutdown` message or a stray ping hitting a `?`-based parser would
kill the read loop, leaving the program running with a silently dead feed —
the worst version of this failure. `Option<Book>` makes "that wasn't a book"
a normal, expected outcome.

### `src/main.rs`

Becomes `#[tokio::main] async fn main() -> anyhow::Result<()>`. After
building `Config` (unchanged from step 0) and initializing telemetry, it
calls into `exchange::binance`'s connect-and-read-loop directly, in the
`main` task — no `spawn`. On each successfully parsed `Book`, log one
`tracing::info!` line:

```
binance ethbtc | bid 0.03150000 x 5.00000000 | ask 0.03151000 x 12.50000000 | id 7723441
```

Full 20-level dumps are never printed — one line per update, best bid/ask
only, via `tracing` (so it respects `ORDERBOOK_LOG_LEVEL` and stays on stderr,
per the existing stdout/stderr split).

### `README.md`

New "Price representation" section under design decisions, carrying:

- The modelling argument (tick multiples, discrete not continuous) stated
  first.
- The measured numbers, presented explicitly as the honest bound on the
  precision benefit, not the justification.
- Where the small error would matter — accumulating arithmetic — named as
  something this step does not do.
- The fixed `1e9`/`i64` scale as a documented decision, with both cost
  bounds (max representable price, smallest tick) stated.
- `exchangeInfo`-derived tick sizes named as the production answer, placed
  under a "what would change for production" note, not implemented here.

## Acceptance Criteria

- `cargo run -- --pair ethbtc` shows live book lines with readable decimal
  prices in a sane range for ETHBTC (~0.03).
- `cargo test` is green, including the 5 parser/conversion tests below.
- `cargo clippy --all-targets -- -D warnings` is clean.
- `cargo fmt --check` is clean.
- `docker compose up --build` runs the same binary the same way.
- A 25+ minute live run, with `tungstenite` at `trace` level, confirms the
  connection survives Binance's 20s ping / 60s pong-timeout without the read
  loop ever writing anything itself — reported with the ping/pong evidence
  from the trace log, not assumed.
- Confirmation (by inspection of the diff) that no `Exchange` trait, no
  `tokio::spawn`, no `.split()`, no reconnection/backoff/staleness, and no
  gRPC code was added.

## Invariants and Critical Don'ts

- No `Exchange` trait or cross-exchange abstraction — one exchange, one
  concrete module; the trait is deferred to step 4 by design, not oversight.
- No `tokio::spawn`, no `.split()` — one feed, one task, driven directly in
  `main`.
- No reconnection, backoff, or staleness logic (step 6) and no gRPC code
  (`server.rs`, generated `orderbook` types) — this step ends at a printed
  line, not a stream.
- `parse` returns `Option<Book>`, never `Result` with a `?`-propagated error
  that would kill the read loop on a non-book message.
- `f64` appears in exactly one place in this step's code: `Display` for
  `Price`/`Amount`. No other arithmetic or comparison touches `f64`.
- The "why not `f64`" comment must state the modelling argument first, the
  measured numbers as an honest bound second, and the accumulation case
  third — not lead with "`f64` can't represent decimals."
- Logs stay on stderr via `tracing` (existing telemetry split) — no
  `println!`.

## Risks and Tradeoffs

- **The ping/pong survival claim is unverified until the 25-minute run
  actually happens.** `tokio-tungstenite` queues pong responses automatically,
  but only sends them when the write half makes progress — and this loop
  never writes. If the connection drops mid-run, that's a real bug found on
  day one rather than at hour forty of a later step, and it blocks moving on
  until fixed (this step's read loop has no reconnection, so a drop simply
  ends the program).
- **No reconnection means any drop — expected 24h force-close, this
  ping/pong risk, or a network blip — ends the process.** Acceptable for this
  step; step 6 owns the fix. Not a regression to chase now.
- **Fixed `1e9` scale is asserted, not derived per-pair.** Correct for
  ETHBTC-scale prices; a pair with a very different order of magnitude is out
  of this step's verification scope (the README states the bound, it isn't
  re-verified per possible pair).

## Testing Strategy

All five tests are pure — no websocket, no mocking, per the project's
narrowest-meaningful-verification-first rule. A real captured Binance
message is embedded as a string literal fixture, since it's the closest
thing to a real code path available without a live socket in a unit test.

- Verify a real captured Binance `depth20` payload parses to exactly 20 bids
  and 20 asks, asserting actual converted `Price`/`Amount` values (not just
  counts) against the source strings.
- Verify `{"e":"serverShutdown","E":1234567890}` parses to `None` without
  panicking.
- Verify malformed JSON parses to `None` without panicking.
- Verify a price round-trip: `"0.03150000"` parses to the integer
  `31500000` and `Display`s back as `0.03150000`.
- Add a regression test — named to say what it guards against (e.g.
  `f64_would_lose_this_precision` or similar) — asserting that
  `0.031505 - 0.031500` computed in the integer domain equals exactly `5000`
  ticks (`5e-06` at this scale), so a future change that "simplifies" `Price`
  to `f64` fails this test rather than silently reintroducing drift.

Beyond the five pure tests, the empirical 25-minute connection run (see
Acceptance Criteria) is required real-world verification of the async/socket
behavior itself — the pure tests cover parsing and price conversion, not
connection survival, which cannot be proven by a unit test.

## Rollback Plan

This step only adds `src/model.rs`, `src/exchange/mod.rs`,
`src/exchange/binance.rs`, and changes `src/main.rs`/`src/lib.rs` (new module
declarations, `async fn main`) plus `README.md`. If acceptance criteria fail
after landing, `git revert` the commit(s) this step produces restores step
0's synchronous, feed-free `main.rs` exactly.

## Open Questions

None blocking — the input brief is prescriptive on every design point in
this step (price scale, newtype shape, message-variant handling, output
format, test list). The one item this spec treats as required verification
rather than an assumption is the ping/pong survival behavior (see Acceptance
Criteria and Risks) — it is empirically checked as part of this step, not
deferred.
