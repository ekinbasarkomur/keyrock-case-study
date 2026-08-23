---
spec_name: "Step 4 — Bitstamp, behind an Exchange trait"
spec_id: "006"
spec_folder: "006-bitstamp"
status: "approved"
created_at: "2026-08-23"
updated_at: "2026-08-23"
created_by: "spec-synthesizer"
creation_mode: "human-brief"
source_inputs:
  - "inputs/001-step-4-brief.md"
source_agents: []
goal: "Two ordered jobs on one branch: (A) refactor the aggregator's single named venue field to a BTreeMap<Venue, VenueState>, no behaviour change; (B) add a Bitstamp feed behind a new synchronous Exchange trait, driven by one generic run_feed<E> loop shared with Binance."
purpose: "A per-venue signature (merge(a), merge(a, b), merge(a, b, c)...) would ripple through step 5 as venues are added; the map fixes the signature now. Bitstamp is the second Exchange implementation, so the trait's shape is observed rather than guessed — deliberately deferred since step 1 for exactly this reason."
parent_request: "step-4 brief, 2026-08-23 (specs/006-bitstamp/inputs/001-step-4-brief.md)"
related_paths:
  - "src/aggregator.rs"
  - "src/merge.rs"
  - "src/exchange/mod.rs"
  - "src/exchange/binance.rs"
  - "src/exchange/bitstamp.rs"
  - "src/feed.rs"
  - "src/main.rs"
  - "README.md"
verification_level: "mixed"
complexity: "medium"
---

# Spec: 006-bitstamp

## Problem

Step 3 (`005-aggregator`) wired one venue end to end: Binance feed → mpsc →
aggregator → pure `summarise()` → `watch` → gRPC. This is step 4 of 11:
add Bitstamp as a second venue.

## Structure: two jobs, in order, on separate commits

**This is the load-bearing fact of this step — everything else is detail.**

- **Job A** — refactor the aggregator's single named field to a
  `BTreeMap<Venue, VenueState>`. No behaviour change. Independent of
  Bitstamp; could be done with one venue today.
- **Job B** — add Bitstamp behind a new `Exchange` trait.

**Why the order and the split matter:** after Job A's commit, every
existing test must pass with no assertion changed (call sites may change
where `summarise`'s new signature forces it — see Acceptance Criteria for
the exact wording). If an assertion needs editing, the refactor changed
behaviour — and that has to be knowable without Bitstamp's arrival on top
of it. One commit for both would mean any breakage is ambiguous between
"the refactor" and "the new venue."

## Job A — the venue map

- `Aggregator { binance: Option<VenueState> }` becomes
  `Aggregator { venues: BTreeMap<Venue, VenueState> }` — fixes `merge`'s
  signature now so adding venues never changes it again (was
  `merge(a)` → `merge(a, b)` → `merge(a, b, c)...` with named fields).
- `BTreeMap`, not `HashMap` — `HashMap` iteration order is unspecified and
  varies run to run; when the tie-break rule (equal price, equal amount)
  has to fall back on iteration order, that fallback must be deterministic
  or the test is flaky, not wrong.
- `Venue` derives `PartialOrd`/`Ord`; the ordering comes from variant
  declaration order in the enum.
- `summarise` receives `BTreeMap<Venue, &Book>` — borrowed, not
  `VenueState`. `VenueState` carries `last_update`, which is clock data,
  and `summarise` must stay clock-free — **inherited from step 3's
  decision 6, not re-derived here.**
- The aggregator builds the borrowed map itself:
  `self.venues.iter().map(|(v, s)| (*v, &s.book)).collect()`.
- **Forward note for step 6:** staleness filtering adds exactly one
  `.filter()` to that chain; `summarise` never learns a clock exists.
  When that filter is written, hoist `Instant::now()` out of the closure
  so every venue is judged against the same instant — not a performance
  concern at two venues, a correctness one (one venue judged fresh, another
  stale, from the same tick).

### `summarise`'s multi-entry behaviour this step — resolved during spec review

Real merging is step 5's work, not this step's. `summarise` still takes the
`BTreeMap<Venue, &Book>` (that's Job A's signature change, settled), but
this step it does the minimum the map shape requires: reads the map's
first (lowest-ordered — Binance, given declared enum order) entry via plain
iteration and summarises that, nothing more. **Deliberately not framed as
"defensively ignoring extra venues"** — no branch, no comment about
guarding against a caller mistake, no silent-drop logic worth flagging. It's
the natural shape of "take the first thing" on a map that may hold more
than one entry once Bitstamp is also feeding it, and step 5 replaces this
internal selection with real merging outright. gRPC output stays
single-venue this step, matching the brief's acceptance criterion.

## Job B — Bitstamp behind an `Exchange` trait

- **Trait**, synchronous:
  `venue() -> Venue`, `connect_url(&self, pair: &str) -> String`,
  `subscribe_message(&self, pair: &str) -> Option<String>`,
  `parse(&self, raw: &str) -> Option<Book>`. Sync because the trait
  describes protocol *data* differences, not control flow — an async
  `connect` would give each venue its own loop, and step 6's reconnection
  would then need to land in two places instead of one.
- **Generic, not `dyn`** — the venue set is compile-time-known; dynamic
  dispatch buys nothing here and costs an indirection. **One line on the
  trade:** generic means `run_feed::<E>` is monomorphised — compiled once
  per concrete `E`, so twice, once for Binance and once for Bitstamp. Zero
  indirection at runtime, a small code-size duplication at compile time;
  the loop is small, so that trade is clearly worth it here.
- **One driver loop, and it is the bulk of Job B — not two spawn calls.**
  `async fn run_feed<E: Exchange>(exchange: E, pair: String, tx:
  Sender<(Venue, Book)>)` absorbs everything `src/main.rs`'s `run_feed`
  currently does by hand for Binance alone: the proxy-vs-direct branch
  (`connect_async` vs `client_async_tls` through a CONNECT tunnel), the
  `Message` variant handling (`Text`/`Ping`/`Pong`/`Close`/`Binary`/`Frame`),
  and the send into the `mpsc`. `subscribe_message` is driven by an
  `if-let` — Binance returns `None` and skips the send, Bitstamp returns
  `Some` and sends it. The loop never branches on which venue it's driving.
  `src/main.rs` shrinks to constructing two `Sender` clones, spawning
  `run_feed::<Binance>` and `run_feed::<Bitstamp>`, and folding both
  `JoinHandle`s into the existing `select!` (a fourth arm, not a
  restructuring of the supervision pattern itself).
- **`run_feed` lives in its own module, `src/feed.rs`, not
  `src/exchange/mod.rs`.** A driver loop isn't exchange-specific data — it's
  the thing that drives *any* `Exchange` impl — so it doesn't belong beside
  the trait and its two implementations. `src/exchange/` stays "the trait
  plus what varies per venue"; `src/feed.rs` is "the one loop that doesn't
  vary."
- **The proxy branch moves into `run_feed` as-is**, since it's shared by
  construction once the loop is generic — Binance needs it, Bitstamp gets
  it for free. **Confirmed working, not assumed:** a manual `CONNECT
  ws.bitstamp.net:443` through the configured proxy during spec review
  returned `200 Connection established` — port 443 is already covered by
  the proxy's default `SSL_ports` ACL (unlike Binance's 9443, which needed
  the explicit ACL addition in step 1). No proxy-side change needed for
  Bitstamp.
- **Endpoint/subscribe**: `wss://ws.bitstamp.net` (nothing in the path);
  after connecting, send
  `{"event":"bts:subscribe","data":{"channel":"order_book_<pair>"}}`.
  Messages arrive wrapped (`{"event":..., "channel":..., "data":{...}}`),
  unlike Binance's flat payload — the inner bid/ask string pairs reuse
  `Price::parse`/`Amount::parse` unchanged. Keep the borrowed
  deserialisation pattern from step 3: the envelope struct borrows out of
  the message the way `Depth20<'a>` does; `Book` still holds nothing
  borrowed.
- **Four event types**, only one is a book:
  - `"data"` → `parse` it into a `Book`.
  - `"bts:subscription_succeeded"` → `None`, log at info.
  - `"bts:request_reconnect"` → `None`, log at info, note that step 6 owns
    turning this into an actual reconnect trigger — not this step.
  - `"bts:error"` → `None`, log at **warn** — this is the one event that
    means something is actually wrong; it must not be logged at the same
    level as the benign ones.
  - Non-book → `None` (never an `Err`) is exactly why `parse` returns
    `Option`, not `Result` — a stray control message must not kill the
    read loop.
- **No `split()` on the socket.** The subscribe is a single write before
  the read loop starts, so read and write never overlap; `split()` would
  cost a mutex on the shared socket for nothing. It would matter if we sent
  periodic pings ourselves or subscribed dynamically at runtime — we don't;
  `tungstenite` already answers Binance's pings automatically, confirmed by
  a 25-minute live run.
- **Symbol formatting stays inside `connect_url`/`subscribe_message` per
  implementation**, not centralized — `ethbtc` becomes
  `ethbtc@depth20@100ms` in a URL path for one venue and `order_book_ethbtc`
  in a channel name for the other; keeping the conversion local means a
  future venue with different casing changes one implementation, not a
  shared formatter.

### Recorded now, not acted on (step 6's problem)

- Bitstamp sends 100 levels, Binance sends 20 — doesn't matter, both take
  the top 10.
- Bitstamp publishes only on change; Binance publishes every 100ms
  regardless. Silence means failure on Binance but can mean a quiet market
  on Bitstamp — **this is exactly why step 6 needs a per-venue staleness
  threshold rather than one shared one.**
- Bitstamp carries a `microtimestamp`; Binance carries no event time at
  all. Not added to `Book` this step — `Book` stays venue-agnostic until
  there's an actual use for it. Log it if useful.

## Scope

**IN:** `src/aggregator.rs` (venue-map refactor), a new `Exchange` trait in
`src/exchange/mod.rs`, `src/exchange/bitstamp.rs` (new), `src/feed.rs`
(new — the generic `run_feed::<E>` driver loop, absorbing what `main.rs`
currently does by hand for Binance: the proxy branch, `Message` handling,
the `mpsc` send), `src/main.rs` (shrinks to spawning two `run_feed::<E>`
instances plus the existing aggregator/server tasks under one `select!`),
tests, README.

**OUT, with the step each lands in:**
- Real two-book merge — step 5.
- Reconnection, and the per-venue staleness thresholds this step's
  cadence difference motivates — step 6.
- A `microtimestamp` field on `Book` — no step assigned yet; log-only if
  useful for now.

## Tests

Every test below names the bug it catches, per this project's testing
convention — no test beyond this list.

**Bitstamp parse, all pure:**
- A real captured `"data"` message parses to the right levels and prices.
- `bts:subscription_succeeded` → `None`, no panic.
- `bts:request_reconnect` → `None`, no panic.
- `bts:error` → `None`, no panic.
- Malformed JSON → `None`, no panic.

**Trait behaviour:**
- Binance's `subscribe_message` is `None`.
- Bitstamp's `subscribe_message` contains the right channel name for the
  configured pair — catches a wrong channel name, which is a silent
  failure: Bitstamp accepts the subscription and then sends nothing.

**Fixture constraint, hard, not a suggestion:** the `"data"` fixture must
be a real captured Bitstamp message, not fabricated — step 1 shipped a
fabricated Binance fixture with visibly synthetic, perfectly regular price
steps, and that is the known mistake this constraint exists to prevent.
Capture it during implementation, trim if unwieldy, and comment where and
when it was captured. If Bitstamp isn't reachable from the implementation
environment, leave a `TODO` naming the user rather than inventing
plausible-looking data.

**Watch-channel test hazard (binding note, not new):** any test driving
the `watch` channel must interleave sends with reads, never batch sends up
front — `watch` only holds the latest value, so two sends before a
subscriber reads collapse into one and the second read never wakes. This
was a real deadlock in step 3 (`specs/005-aggregator/revisions.md`, entry
3), not a hypothetical.

## Acceptance Criteria

- Both venues' book lines scrolling in the logs.
- `grpcurl` still streaming — single venue (merge is step 5).
- **Job A's commit: no assertion changes, even though some call sites
  must.** `summarise`'s signature changes (`summarise(Venue::Binance,
  Some(&book))` → `summarise(&BTreeMap::from([(Venue::Binance, &book)]))`),
  so `merge.rs`'s own unit tests must edit their setup lines to keep
  compiling — that's expected and not a violation. What must **not**
  change is any `assert_eq!`/`assert!` value or condition in any test,
  anywhere. If an assertion needs editing to pass, the refactor changed
  behaviour, and that's exactly what this criterion exists to catch —
  loosening it to "tests still pass" would let a real behaviour change
  slip through unnoticed.
- The Bitstamp fixture is real, with capture provenance in a comment.
- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all pass.
- `docker compose up` brings up both feeds connecting.
- `git diff main --stat -- src/merge.rs` shows only Job A's signature
  change — zero additional diff from Job B. This is this step's scope
  check.

## Open Questions

None blocking. `summarise`'s multi-entry behaviour (see Job A section
above) was resolved during spec review: plain first-entry iteration inside
`summarise`, no defensive/ignore-extras framing.

Full reasoning behind every settled decision above (BTreeMap vs HashMap,
sync vs async trait, generic vs `dyn`, no `split()`, symbol formatting
placement) lives in the user's own handbook outside this repo, per this
step's explicit "two minutes of reading" instruction — not re-derived here
even if a future reader wants more than the one-line why.
