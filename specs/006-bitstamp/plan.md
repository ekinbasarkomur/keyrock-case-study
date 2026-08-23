# Plan: 006-bitstamp

## Summary

Five phases, sized above `005-aggregator`'s five (`complexity: small`) but not
padded to match — this step's `complexity: medium` comes from genuinely more
moving parts (a `BTreeMap` refactor plus a second `Exchange` implementation
plus a new generic driver loop), not from splitting the same amount of work
into more commits. The one non-negotiable structural fact, repeated from
spec.md because it governs every phase boundary below: **Job A (the venue-map
refactor) is entirely done, verified, and its own commit before any Job B
(Bitstamp) work starts.** Phase 1 is Job A, alone. Phases 2-4 are Job B, split
into three sub-phases rather than one, because Job B itself has an internal
risk order worth isolating the same way `005-aggregator`'s plan isolated its
one genuine risk (borrowed deserialisation) before building on top of it:

- Phase 2 wraps Binance into the new `Exchange` trait and lands the generic
  `run_feed<E>` driver — but wires `main.rs` to run **only** `Binance`
  through it. This proves the generic loop itself works (monomorphisation,
  the URL-derived proxy target, the `if-let` subscribe branch) against a
  known-good venue before a second, newly-written parser is layered on top.
  If the generic refactor broke something, this phase's regression check
  (`grpcurl` still streaming Binance data) catches it in isolation.
- Phase 3 adds `src/exchange/bitstamp.rs` — a new `Exchange` impl plus its
  five parse tests and its `subscribe_message` test — entirely unwired.
  Nothing in `main.rs` changes. This is the "prove it pure, before it's
  live" phase, mirroring `005-aggregator`'s phase-2 discipline of proving
  `summarise()` against hand-built fixtures before anything runs it.
- Phase 4 is the actual wiring: a second `run_feed::<Bitstamp>` spawn, folded
  into the existing `select!` as a fourth arm. This is where "both venues'
  book lines in the logs" first becomes true, and it is deliberately the
  last code-changing phase — everything upstream of it (Job A's map, the
  generic driver, Bitstamp's parser) is already independently proven, so a
  failure here is isolated to the wiring itself.

Phase 5 is the README pass, landing last per the standing rule from
`specs/003-step-1-fixes/revisions.md` entry 1 (a step's README describes what
was actually shipped, not what was planned).

One structural payoff worth stating up front because it shapes what Phase 4
does *not* need to touch: Job A's `BTreeMap<Venue, VenueState>` refactor means
`src/aggregator.rs`'s update path becomes a generic `.insert(venue, ...)` —
no `match venue { ... }` arms to extend. Confirmed against the current
`src/aggregator.rs` (read before writing this plan): today it still has a
real `match venue { Venue::Binance => ... }`, which is exactly what Job A
replaces with a map insert. This is the reason Phase 3 (Bitstamp's parser)
and Phase 4 (wiring) should show **zero diff** to `src/aggregator.rs` — if
either phase finds itself needing to touch that file, Job A's refactor did
not fully deliver on its own stated purpose ("adding venues never changes
`merge`'s signature again") and that's worth flagging before continuing, not
patching around.

## Phase Breakdown

### Phase 1 (Job A): the venue map — `src/aggregator.rs`, `src/merge.rs`, `src/exchange/mod.rs`

- Objective: Land the refactor spec.md calls out as independently
  commit-able, with no behaviour change, before Bitstamp exists to make any
  breakage ambiguous between "the refactor" and "the new venue."
- Main changes:
  - `src/exchange/mod.rs`: `Venue` gains `#[derive(PartialOrd, Ord)]` on top
    of its existing derives (`Clone, Copy, Debug, PartialEq, Eq`) — checked
    against the current file, which has exactly those five and one variant
    (`Binance`). No new variant yet; `Bitstamp` is Phase 3's addition, kept
    out of Job A on purpose (spec.md frames Job A as "independent of
    Bitstamp; could be done with one venue today"), and the declaration
    order established now (`Binance` first) is what fixes `BTreeMap`
    iteration order once `Bitstamp` is appended later.
  - `src/aggregator.rs`: `Aggregator { binance: Option<VenueState> }` becomes
    `Aggregator { venues: BTreeMap<Venue, VenueState> }`. The `run` loop's
    body changes from the current `match venue { Venue::Binance => ... }`
    block to a single `aggregator.venues.insert(venue, VenueState { book,
    last_update: Instant::now() });` — generic over any `Venue`, which is
    the concrete mechanism behind this plan's Summary claim that Phase 3/4
    need zero further changes here. After the insert, build the borrowed map
    per spec.md's exact line — `let venues: BTreeMap<Venue, &Book> =
    aggregator.venues.iter().map(|(v, s)| (*v, &s.book)).collect();` — and
    call `merge::summarise(&venues)`.
  - `src/merge.rs`: `summarise`'s signature changes from `summarise(venue:
    Venue, book: Option<&Book>) -> Option<Summary>` to `summarise(venues:
    &BTreeMap<Venue, &Book>) -> Option<Summary>`. Body change is mechanical:
    `let (&venue, &book) = venues.iter().next()?;` replaces the current
    `let book = book?;`, then everything downstream (`best_bid`/`best_ask`/
    `to_level`'s `exchange: venue.to_string()`) is unchanged, reading `venue`
    and `book` from that binding instead of from parameters. Per spec.md's
    explicit resolution: **no comment framing this as "ignoring extra
    entries," no branch for a multi-entry map** — it's plain first-entry
    iteration, nothing else, because step 5 replaces this selection outright.
  - `src/merge.rs`'s five existing tests: setup lines change (each call site
    builds `&BTreeMap::from([(Venue::Binance, &book)])` instead of passing
    `Venue::Binance, Some(&book)` as two arguments; the "no book" test builds
    `&BTreeMap::new()` instead of passing `None`). **No `assert_eq!`/`assert!`
    value or condition may change** — this is the phase's actual acceptance
    gate, not "tests still pass."
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — all five `src/merge.rs` tests pass with their assertions
    byte-for-byte unchanged from the current file (diff the test bodies
    against what was read before this plan was written, not just "green").
  - `git diff main --stat -- src/merge.rs` — record the output now as this
    phase's baseline. It should show a small diff confined to the function
    signature, the one new `let (&venue, &book) = ...` line, and the five
    tests' setup lines — nothing else. This baseline is what Phase 5's final
    gate re-confirms is unchanged after Job B lands.
  - Inspection: `src/aggregator.rs` has no `match venue { ... }` left in
    `run` — a leftover match would mean the map refactor didn't actually
    remove per-venue branching, defeating Job A's stated purpose.
- Done looks like: the crate builds and tests green with the exact same
  assertions as before this phase, `Aggregator` and `summarise` both speak
  `BTreeMap` end to end, and there is nothing anywhere in this diff that a
  reviewer could point to as a behaviour change rather than a reshape.
- Commit boundary: `src/aggregator.rs`, `src/merge.rs`, `src/exchange/mod.rs`.
  This is Job A's commit, standalone. Reverting it restores the single
  `Option<VenueState>` field and the two-argument `summarise`, with nothing
  yet depending on the map shape.

### Phase 2 (Job B, part 1): the `Exchange` trait, Binance wrapped, `src/feed.rs` — Binance-only regression

- Objective: Land the riskiest structural piece of Job B — a generic,
  monomorphised driver loop — against a venue whose correct output is
  already known, before a second, newly-written parser (Bitstamp) is
  layered on top of an unproven abstraction. If the generic refactor itself
  broke something, this phase's `grpcurl` check catches it without Bitstamp
  in the picture to confuse the diagnosis.
- Main changes:
  - `src/exchange/mod.rs`: new `pub trait Exchange { fn venue(&self) ->
    Venue; fn connect_url(&self, pair: &str) -> String; fn subscribe_message
    (&self, pair: &str) -> Option<String>; fn parse(&self, raw: &str) ->
    Option<Book>; }` — synchronous, per spec.md's explicit reasoning (the
    trait describes protocol data, not control flow; an async `connect`
    would give reconnection two places to land in step 6 instead of one).
  - `src/exchange/binance.rs`: add `pub struct Binance;` and `impl Exchange
    for Binance`. The existing `connect_url(pair: &str) -> String` and
    `parse(text: &str) -> Option<Book>` free functions' *logic* moves into
    the trait methods unchanged — this is a reshape of Binance's public
    shape, not a behaviour change, mirroring Job A's own "call sites may
    change, assertions may not" discipline (spec.md draws this parallel
    explicitly). `subscribe_message` returns `None` — Binance's subscription
    is baked into the URL. `HOST`/`PORT` constants stay as they are (still
    used to build the URL inside `connect_url`).
  - `src/exchange/binance.rs`'s three existing tests: call sites change from
    the free-function form (`parse(DEPTH20_FIXTURE)`) to the trait-method
    form (`Binance.parse(DEPTH20_FIXTURE)` or equivalent) — assertions stay
    identical. One new test is added here per spec.md's trait-behaviour
    list: `binance_subscribe_message_is_none`, filed here (not in
    `exchange/mod.rs`) per this project's unit-test-by-access convention —
    it needs nothing beyond what's already `pub` in this file.
  - `src/feed.rs` (new): `pub async fn run_feed<E: Exchange>(exchange: E,
    pair: String, tx: mpsc::Sender<(Venue, Book)>) -> Result<()>` — absorbs,
    unchanged in behaviour, everything the current `src/main.rs::run_feed`
    does by hand for Binance: the proxy-vs-direct branch, the `Message`
    variant match (`Text`/`Ping`/`Pong`/`Close`/`Binary`/`Frame`), and the
    `mpsc` send — now sending `(exchange.venue(), book)` instead of a
    hardcoded `(Venue::Binance, book)`. After connecting, `if let Some(msg) =
    exchange.subscribe_message(&pair) { ws.send(...).await?; }` — Binance's
    `None` skips the send, a future Bitstamp's `Some` sends it. The log
    line inside the read loop switches from the current hardcoded `"binance
    {}"` format string to interpolating `exchange.venue()`, so it reads
    correctly for whichever concrete `E` this instantiation is. **See "Plan
    Review Notes" below for how the proxy `CONNECT` target's host/port are
    derived generically** — this was not fully specified by spec.md's trait
    shape and had to be resolved here.
  - `proxy_addr()` (the `HTTPS_PROXY`/`HTTP_PROXY` env-var reader, currently
    a free function in `src/main.rs`) moves into `src/feed.rs` as a private
    function, since `run_feed` is now the only caller and `main.rs` is
    meant to shrink to construction and spawning, not helper functions.
  - `src/main.rs`: the old `run_feed` function is deleted; the feed spawn
    becomes `tokio::spawn(feed::run_feed(binance::Binance, pair, feed_tx))`.
    **Still single-venue this phase** — Bitstamp isn't spawned until Phase
    4 — so this is a structural/behavioural no-op from the outside, proven
    by the same `grpcurl` check Phase 3 of `005-aggregator` used.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — the three existing Binance parser tests pass with
    assertions unchanged (only their call-site syntax differs), plus the
    new `subscribe_message`-is-`None` test.
  - `cargo run -- --pair ethbtc --port 50051`, then `grpcurl -plaintext
    127.0.0.1:50051 orderbook.OrderbookAggregator/BookSummary` (network
    permitting — report honestly if this environment can't reach Binance,
    same standing rule `004-grpc-server`'s and `005-aggregator`'s plans
    both used) — confirm the stream still carries real Binance data with
    `Level.exchange == "binance"`, proving the generic loop reproduces the
    pre-refactor behaviour exactly.
  - Inspection: `git diff main --stat -- src/aggregator.rs` shows no diff
    beyond Phase 1's — this phase touches the feed side only.
- Done looks like: the crate builds, Binance's tests pass unchanged, and a
  live `grpcurl` session against `run_feed::<Binance>` looks identical to
  what `005-aggregator` shipped — proving the trait/generic-loop reshape
  carries zero behavioural risk before Bitstamp is layered on top of it.
- Commit boundary: `src/exchange/mod.rs`, `src/exchange/binance.rs`,
  `src/feed.rs`, `src/main.rs`. Reverting this phase (with Phase 1 still in
  place) restores the hand-written single-venue `run_feed` in `main.rs`, on
  top of an already-landed, independently-verified `BTreeMap` aggregator.

### Phase 3 (Job B, part 2): `src/exchange/bitstamp.rs` — pure, unwired

- Objective: Prove Bitstamp's parser correct against a real fixture before
  anything in `main.rs` depends on it — the same "pure first, wired later"
  ordering `005-aggregator`'s plan used for `summarise()`.
- Main changes:
  - `src/exchange/mod.rs`: `Venue` gains a `Bitstamp` variant, appended
    after `Binance` (declaration order — `Binance` stays first, preserving
    Phase 1's `BTreeMap` ordering guarantee). `Display` gains the matching
    arm (`Venue::Bitstamp => write!(f, "bitstamp")`).
  - `src/exchange/bitstamp.rs` (new): `pub struct Bitstamp;` implementing
    `Exchange`. `connect_url` returns `"wss://ws.bitstamp.net"` regardless
    of `pair` (nothing in the path per spec.md — the pair only shows up in
    the subscribe channel name). `subscribe_message` returns
    `Some(format!(r#"{{"event":"bts:subscribe","data":{{"channel":"order_book_{}"}}}}"#,
    pair.to_lowercase()))` (symbol casing/formatting stays local to this
    impl per spec.md, not centralised). `venue()` returns `Venue::Bitstamp`.
  - `parse(&self, raw: &str) -> Option<Book>`: deserialises the wrapped
    envelope (`{"event":..., "channel":..., "data":{"timestamp":...,
    "microtimestamp":...,"bids":[[...]],"asks":[...]}}`) using the same
    `#[serde(borrow)]` pattern `Depth20<'a>` uses in `binance.rs` — an inner
    `Data<'a>` struct borrowing `bids`/`asks` as `Vec<[&'a str; 2]>`, reused
    through `Price::parse`/`Amount::parse` exactly as Binance's parser does,
    so the returned `Book` holds nothing borrowed. Branches on the
    envelope's `event` field: `"data"` → parse into `Book`; `"bts:subscription_succeeded"`
    → `None` + `tracing::info!`; `"bts:request_reconnect"` → `None` +
    `tracing::info!` + a comment noting step 6 owns turning this into an
    actual reconnect trigger; `"bts:error"` → `None` + `tracing::warn!` (not
    `info!` — this is the one event that means something is actually
    wrong); any other/unrecognised shape (including malformed JSON) → `None`,
    never a panic or `Err`.
  - **Fixture capture, real work item of this phase, not a formality:**
    attempt a real captured `"data"` message from `wss://ws.bitstamp.net`
    before writing the test — e.g. a short-lived debug connection (a
    throwaway `cargo run --example` or a temporary `#[tokio::test]` printed
    to stdout and then deleted), the same pattern `002-binance-feed` used to
    capture its own Binance fixture (see that packet's history/revisions
    for the precedent — the constraint exists specifically because step 1
    shipped a visibly synthetic fixture once already). Trim the captured
    payload if unwieldy, keep it valid JSON, and comment where/when it was
    captured (mirroring `binance.rs`'s existing fixture comment: `// Captured
    from wss://... on <date>`). **Fully revert any temporary debug code**
    used to capture it — it must not survive into the commit. If Bitstamp
    is genuinely unreachable from the implementation environment, the
    fixture and its test are replaced with a `TODO` comment naming the user
    and explaining what's blocked — never a fabricated "looks real" payload.
    This is a hard constraint from spec.md, not a suggestion, and this
    plan's phase gate below checks it explicitly.
  - Tests, all pure, co-located `#[cfg(test)] mod tests` in `bitstamp.rs`
    per this project's unit-test-by-access convention:
    - the real `"data"` fixture parses to the right levels/prices (bug
      caught: a wrong field path into the wrapped envelope, e.g. reading
      `bids` at the top level instead of inside `data`)
    - `bts:subscription_succeeded` → `None`, no panic (bug caught: treating
      a benign lifecycle message as a parse failure worth propagating, or a
      panic on a payload shape with no `bids`/`asks` at all)
    - `bts:request_reconnect` → `None`, no panic (bug caught: same class,
      plus confirms this specific event is recognised rather than falling
      through the generic "unknown event" branch silently)
    - `bts:error` → `None`, no panic (bug caught: same class; also worth
      confirming — by reading the log output, not asserting on log
      internals — that this one logs at `warn`, not `info`)
    - malformed JSON → `None`, no panic (bug caught: an unhandled
      `serde_json` error propagating instead of being converted to `None`)
    - `bitstamp_subscribe_message_contains_the_configured_pairs_channel_name`
      — asserts the returned string contains `"order_book_ethbtc"` for pair
      `"ethbtc"` (bug caught, per spec.md: a wrong channel name is a silent
      failure — Bitstamp accepts the subscription and then sends nothing,
      which looks identical to "no messages yet" from the outside)
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — all six tests above pass, named as behaviour sentences,
    reported individually (not folded into an aggregate count) per this
    project's testing convention.
  - Inspection: the fixture's capture-provenance comment is present and
    honest (a real date/URL, not invented) — or, if capture genuinely
    failed in this environment, a `TODO` naming the user is present instead
    of a fixture, and this is reported explicitly as a phase outcome, not
    silently worked around.
  - `git diff main --stat -- src/aggregator.rs src/merge.rs` — zero diff.
    This phase adds a new file and touches only `src/exchange/mod.rs`
    (the new `Venue` variant); nothing about parsing a second venue should
    ripple into the aggregator or merge logic, confirming the Summary's
    stated structural payoff.
- Done looks like: `Bitstamp` is a fully-tested `Exchange` implementation
  that nothing in `main.rs` references yet — provably correct in isolation,
  same as `summarise()` was before it was wired up in `005-aggregator`.
- Commit boundary: `src/exchange/bitstamp.rs`, `src/exchange/mod.rs`.
  Reverting this phase (with Phases 1-2 still in place) leaves Binance
  running through the generic loop alone, exactly Phase 2's end state.

### Phase 4 (Job B, part 3): wire Bitstamp into `src/main.rs`

- Objective: The actual point of Job B — both venues feeding the aggregator
  concurrently. Deliberately the last code-changing phase, so a failure here
  is isolated to the wiring itself, not confused with a parser bug or a
  generic-loop bug that Phases 2-3 already ruled out independently.
- Main changes:
  - `src/main.rs`: `feed_tx` is cloned once more (one `Sender`, cloned for
    two producers, per spec.md's exact framing); a second
    `tokio::spawn(feed::run_feed(bitstamp::Bitstamp, pair, feed_tx))` is
    added alongside the existing Binance spawn. The `select!` gains a
    fourth arm for this new `JoinHandle`, following the same
    match-on-`Ok`/`Err`/panic shape the other three arms already use — this
    is explicitly *not* a restructuring of the three-arm supervision
    pattern, just one more task under the same discipline (any one task
    ending ends the whole process).
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo run -- --pair ethbtc --port 50051` — confirm both venues' book
    lines are visible scrolling in the logs (this environment's Binance
    reachability depends on the proxy per prior steps; Bitstamp is expected
    to be directly reachable from Turkey per spec.md's own confirmed manual
    `CONNECT` test — but this must be **actually observed and reported**,
    not assumed from spec.md's claim; if either venue can't be reached in
    this environment, say so plainly rather than inferring success).
  - `grpcurl -plaintext 127.0.0.1:50051 orderbook.OrderbookAggregator/BookSummary`
    — still streams, single-venue output (`exchange == "binance"`, since
    `summarise` still reads the map's lowest-ordered entry — real merging
    is step 5). This is a regression check: two live feeds now write into
    the same aggregator, and the gRPC surface must look unchanged to a
    client from before this step.
  - `docker compose up --build` — confirm both feed tasks connect (report
    actual observed output, including any environment limitation, per the
    same non-negotiable honesty standard every prior step's plan in this
    repo has used).
- Done looks like: killing either feed still ends the whole process via the
  same four-arm `select!`; both venues' parsed books are visibly reaching
  the aggregator; the gRPC contract is unchanged from a client's
  perspective.
- Commit boundary: `src/main.rs` alone (plus, if the Cargo edition requires
  it, a `pub mod bitstamp;`-style export already added in Phase 3's
  `src/exchange/mod.rs` — confirm no further changes are needed there).
  Reverting this phase (with Phases 1-3 in place) leaves Bitstamp fully
  built and tested but not spawned — a safe, buildable intermediate state.

### Phase 5: `README.md`

- Objective: Describe what Phases 1-4 actually shipped, once there's real
  two-venue behaviour to document, per the standing rule from
  `specs/003-step-1-fixes/revisions.md` entry 1.
- Main changes: `README.md` — build-order table's step 4 row moves to
  "Done"; a short note that the aggregator now holds a `BTreeMap<Venue,
  VenueState>` rather than a single named field, and why (fixes `merge`'s/
  `summarise`'s signature ahead of further venues, per spec.md's own
  framing); the `Exchange` trait, `src/feed.rs`'s generic `run_feed<E>`,
  and `src/exchange/bitstamp.rs` added to the Layout tree; an explicit
  statement that gRPC output is still single-venue this step (real merging
  is step 5) so a reader doesn't mistake "two feeds running" for "two
  venues in the published book"; the Bitstamp fixture's capture provenance
  referenced (or the `TODO` noted, if capture failed in every environment
  tried) so a reader can see the same honesty this plan requires of the
  implementation itself.
- Verification:
  - Manually run every command the README's Quick Start / gRPC sections
    show and confirm actual output matches what's documented, to the extent
    this environment allows (same honesty note as every prior phase).
  - Read-through: no leftover references to a single `binance` field on
    `Aggregator`, no stale "step 4: not yet implemented" language.
- **Full-branch verification gate, run once here at the tip:**
  - `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
    `cargo fmt --check` all clean.
  - `git diff main --stat -- src/merge.rs` — compared against Phase 1's
    recorded baseline. **Must be identical.** Any additional diff means Job
    B touched `merge.rs`, which is out of scope for this step (real merging
    is step 5) and violates spec.md's explicit acceptance criterion.
  - `git diff main --stat -- proto/orderbook.proto src/config.rs
    src/telemetry.rs src/proxy.rs` — zero diff on all four. None of this
    step's work (the venue map, the trait, the second feed) has any reason
    to touch the wire schema, config parsing, telemetry setup, or the
    proxy-tunnel primitives themselves (only their existing `pub` surface is
    reused from `src/feed.rs`).
- Done looks like: every claim in the README matches what was actually
  shipped and actually verified in this environment; both `git diff --stat`
  checks above are clean at the tip of the branch.
- Commit boundary: `README.md` alone. Reverting it has no effect on build or
  test state.

## Cross-Cutting Considerations

- **Job A before Job B, no exceptions, checked structurally not just by
  commit order.** Phases 3 and 4 are both expected to show zero diff to
  `src/merge.rs` and (per the Summary's stated payoff) zero diff to
  `src/aggregator.rs` beyond Phase 1's. If either phase's diff touches
  those files, stop and re-examine whether Job A's refactor actually
  delivered what spec.md claims it does, rather than patching the symptom.
- **`BTreeMap` ordering depends on `Venue`'s declaration order.** `Binance`
  must stay the first variant; `Bitstamp` is appended, never inserted
  before it. This is what makes `summarise`'s "first (lowest-ordered)
  entry" behaviour this step deterministically mean "Binance," not
  something that could flip based on map internals.
- **Proxy `CONNECT` target host/port — resolved here, not fully specified
  by spec.md's trait shape.** See "Plan Review Notes" below.
- **No `split()` on the socket, carried forward into `src/feed.rs`
  unchanged** — the subscribe write happens once, before the read loop
  starts, for both venues; `tungstenite` already answers pings
  automatically, confirmed by the prior step's 25-minute live run.
- **Symbol formatting stays local to each `Exchange` impl.** Neither
  `feed.rs` nor `exchange/mod.rs` gains a shared pair-formatting helper —
  `Binance::connect_url` and `Bitstamp::subscribe_message` each format the
  pair their own way, per spec.md's explicit reasoning (a future venue with
  different casing changes one impl, not a shared formatter).
- **`bts:request_reconnect` is logged, not acted on.** Every phase touching
  `bitstamp.rs` must resist the temptation to actually trigger a reconnect
  here — that's step 6's job, explicitly out of scope, and spec.md is
  explicit that this step only leaves the comment.
- **Watch-channel test hazard carried forward, not triggered this step.**
  `specs/005-aggregator/revisions.md` entry 3's interleave-sends-with-reads
  rule applies to any test driving the `watch` channel. None of this step's
  planned tests do (they're all pure/parse-level, per spec.md's Tests
  section) — noted here so a plan deviation that adds a `watch`-driving
  test later in implementation doesn't rediscover the deadlock.
- **Untouched-files discipline**, same convention `004-grpc-server` and
  `005-aggregator` both used: `proto/orderbook.proto`, `src/config.rs`,
  `src/telemetry.rs`, `src/proxy.rs` should show zero diff at the tip of
  this branch. A phase whose diff unexpectedly touches one of these is a
  stop-and-flag condition.

## Verification Gates

Before this branch is considered ready to hand off:

- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all clean at the tip of the branch.
- `cargo test` reports at least the eleven behaviour-named tests spec.md's
  Tests section implies across this plan (five `merge.rs` tests carried
  over from `005-aggregator` with unchanged assertions, one new Binance
  `subscribe_message`-is-`None` test, six Bitstamp tests — five parse + one
  channel-name), each identifiable individually, not folded into an
  aggregate pass/fail count.
- Both venues' book lines observed scrolling in the logs from a live
  `cargo run`, or an honest statement of which venue(s) this environment
  could not reach.
- `grpcurl -plaintext 127.0.0.1:50051 orderbook.OrderbookAggregator/BookSummary`
  against a live `cargo run` still streams, single-venue, `exchange ==
  "binance"` — actual output quoted, network permitting.
- `docker compose up --build` brings up both feed tasks — actual observed
  output reported, including any environment limitation.
- `git diff main --stat -- src/merge.rs` shows only Job A's signature
  change — checked once at Phase 1's tip as a recorded baseline, and again
  at Phase 5's tip, and the two must be identical.
- `git diff main --stat -- proto/orderbook.proto src/config.rs
  src/telemetry.rs src/proxy.rs` shows zero diff.
- The Bitstamp `"data"` fixture is real, with capture provenance in a
  comment — or a `TODO` naming the user, honestly reported, if capture was
  genuinely unreachable in every environment tried.
- README's claims match what was actually shipped and actually verified in
  this environment, not what was planned.

## Expected Drift Triggers

If any of the following becomes true while implementing, update spec.md
before continuing rather than improvising past it:

- Phase 2 finds that deriving the proxy `CONNECT` target's host/port from
  `connect_url`'s returned string (this plan's proposed resolution — see
  "Plan Review Notes") doesn't generalise cleanly — e.g. a URL shape neither
  venue actually produces, or `wss://` without an explicit port meaning
  something other than 443 in some case. Worth a spec.md update recording
  the actual resolution, not a silent one-off `if venue == Bitstamp` branch
  reintroducing the per-venue branching the generic loop exists to avoid.
- Phase 3's fixture capture is unreachable from the implementation
  environment for both a live debug connection attempt and any reasonable
  retry — the plan requires a `TODO` naming the user in this case, not a
  fabricated "plausible" payload; if this happens, flag it prominently
  rather than letting Job B's test list look complete when it isn't.
- Phase 3 discovers Bitstamp's envelope shape doesn't actually match what
  spec.md described (e.g. `bids`/`asks` nested differently, or a fifth event
  type observed in the wild that spec.md's four-event list doesn't cover) —
  a genuine surprise worth a spec.md update, not a silent fifth branch
  added without recording why.
- Phase 4 discovers the bounded `mpsc::channel(32)` (unchanged from
  `005-aggregator`, now shared by two producers instead of one) genuinely
  fills up with two live feeds running — worth surfacing as a real signal
  about that capacity choice under two-venue load, not quietly raised to
  make the symptom disappear.
- Any phase finds itself touching `src/merge.rs` or adding a `match venue`
  branch back into `src/aggregator.rs` — per this plan's Cross-Cutting
  section, that's a sign Job A's refactor didn't fully deliver its stated
  purpose and needs re-examination, not a normal implementation detail to
  push through.
- `docker compose up` cannot be run at all in this environment (no Docker
  daemon, no route to either exchange even through the configured proxy) —
  report this as "not verified here," not silently omitted, same standing
  rule every prior step's plan in this repo has used.

## Plan Review Notes — resolved before task-writer

One structural detail spec.md's trait shape left unstated, resolved during
plan review (not a spec change — it doesn't contradict anything spec.md
settled, it fills a gap the given trait signature genuinely leaves open):

- **The proxy `CONNECT` tunnel needs a `(target_host, target_port)` pair,
  but the `Exchange` trait spec.md defines only has `connect_url(&self,
  pair: &str) -> String`, which returns a full `wss://host:port/path`
  string, not host and port separately.** Today's `src/main.rs` sidesteps
  this by hardcoding `binance::HOST`/`binance::PORT` constants at the
  `proxy::connect_through_proxy` call site — a per-venue reference that a
  generic `run_feed<E>` cannot keep without reintroducing the branching the
  trait exists to remove. This plan resolves it by parsing the host and
  port back out of whatever `exchange.connect_url(&pair)` returns, inside
  `src/feed.rs`: strip an optional `wss://` prefix, take everything before
  the first `/` as the authority, split that on the last `:` for an
  explicit port, and default to `443` (the `wss` scheme's standard port,
  which is exactly what Bitstamp's `wss://ws.bitstamp.net` — no explicit
  port in the URL — relies on) when there's no `:` in the authority. This
  keeps `run_feed` fully generic and needs nothing new from the `Exchange`
  trait itself, at the cost of one small parsing helper in `src/feed.rs`
  that the task-writer should size as its own few lines, not fold silently
  into the middle of the connect logic. Flagging this explicitly since it
  wasn't spelled out in the brief handed to this planning pass.

No other part of spec.md was found underspecified enough to require a
resolution note — the `Exchange` trait's four methods, the four Bitstamp
event branches, the fixture constraint, and the phase-ordering requirement
were all concrete enough to plan against directly.
