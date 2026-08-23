# Plan: 005-aggregator

## Summary

Five phases, sized to this step's actual scope (`complexity: small` in
spec.md's frontmatter) rather than padded to match `004-grpc-server`'s six.
Ordering follows the dependency chain the spec's own Shape diagram already
implies, with one risk-isolation phase pulled to the front: the Binance
parser's switch to borrowed deserialisation (decision 4) is the one place
this step could genuinely surprise ("if serde can't actually borrow... report
that rather than silently falling back"), so it lands alone first, exactly
the way `004-grpc-server`'s plan isolated its own version-resolution risk
before touching anything downstream. `src/merge.rs`'s `summarise()` lands
second — pure, no dependency on anything except `Book`, and it carries five
of this step's six tests, so proving it in isolation before any wiring
exists is the cheapest place to catch a bug. The third phase is necessarily
one atomic unit rather than three separate ones: `src/aggregator.rs` (new),
`src/server.rs`'s watch-channel type change (`Option<Summary>` →
`Option<Arc<Summary>>`, `run_fake_writer` deleted), and `src/main.rs`'s mpsc
wiring all have to land together because the crate does not compile in any
intermediate state between them — `main.rs` constructs the channels, so a
half-migrated watch type or a still-referenced `run_fake_writer` breaks the
build, not just the tests. `tests/grpc.rs` lands fourth, once there's a real
pipeline to drive it with instead of the placeholder it was built against.
`README.md` lands last, describing what phases 1-4 actually shipped, per the
standing rule from `specs/003-step-1-fixes/revisions.md` entry 1 that a
step's README lands with the step, not after it.

Every phase's diff is checked against `proto/orderbook.proto` (must show
zero diff — see CLAUDE.md's "Don't touch the protobuf schema") and the
`Venue`/fixed-point/`Price`-`Amount` boundary rules already settled in
`src/model.rs` are treated as fixed, not reopened.

## Phase Breakdown

### Phase 1: `src/exchange/binance.rs` — borrowed deserialisation

- Objective: Land the one genuine risk this step carries (decision 4) in
  isolation, before anything downstream depends on `Book` still being
  correctly owned. If serde can't actually borrow here, that needs to be
  known before `src/merge.rs` and `src/aggregator.rs` are built on top of an
  assumption that turned out false.
- Main changes: `Depth20<'a>` replaces the current owned `Depth20` struct —
  `bids`/`asks` become `Vec<[&'a str; 2]>` with `#[serde(borrow)]` instead of
  `Vec<[String; 2]>`. `parse_levels` and `parse` themselves change only
  enough to thread the borrow through `Price::parse`/`Amount::parse` (both
  already take `&str`, per spec.md's Open Questions — expected to just work,
  not re-derived here). The borrow must end inside `parse()`: the returned
  `Book` holds only `Price`/`Amount` (`f64`, `Copy`) — nothing borrowed
  survives past the function boundary, which is what lets `Book` travel down
  an `mpsc` in phase 3 without a lifetime attached to it. `src/exchange/mod.rs`
  gains `enum Venue { Binance }` plus a small label impl (`Display` or
  equivalent yielding `"binance"`) — see "Plan Review Notes" below for why
  it lands here rather than in phase 3.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — the three existing unit tests in this file
    (`parses_depth20_into_twenty_bids_and_twenty_asks_with_correct_values`,
    `server_shutdown_message_parses_to_none_without_panicking`,
    `malformed_json_parses_to_none_without_panicking`) must pass unchanged;
    this phase changes internal representation, not parsed output, so no
    test's assertions should need editing — only failing to compile would
    indicate the borrow actually leaked.
  - Inspection: confirm `Depth20` is declared with `&'a str` (not `String`)
    at every field — this is also acceptance criteria's second bullet,
    verified here rather than deferred.
- Done looks like: the crate builds, the three existing parser tests pass
  without modification, and the struct genuinely borrows — not "the code
  looks like it borrows but an intermediate `.to_string()` defeats it."
- Commit boundary: `src/exchange/binance.rs`, `src/exchange/mod.rs`.
  Reverting this phase restores the owned-`String` parser and removes
  `Venue`, with nothing downstream yet depending on either.

### Phase 2: `src/merge.rs` — `summarise()`

- Objective: The pure core of this step, buildable and testable with zero
  dependency on channels, tasks, or the gRPC layer — construct `Book` values
  by hand, per this project's testing convention for pure functions.
- Main changes: `src/merge.rs` (new) — `pub fn summarise(venue: Venue, book:
  Option<&Book>) -> Option<Summary>` per decision 6: no clock, no I/O, no
  channel reference anywhere in this file. `venue` sources each `Level`'s
  `exchange` label (see "Plan Review Notes" below) — no hardcoded `"binance"`
  literal in this file. Takes the first 10 levels of each side (Binance
  already hands over a sorted book, so this step is truncation, not sorting
  — the real sort/merge work is step 5's `merge()`, which is why this
  function's name and this file are already shaped for a two-book signature
  later without a rename). `src/lib.rs` gains `pub mod merge;`.
- Tests (all five of spec.md's `summarise`-specific tests belong here,
  co-located `#[cfg(test)] mod tests`, hand-built `Book` fixtures per this
  project's unit-test filing convention):
  - a 20-level book → 10 bids, 10 asks, correct spread
  - bids come back descending by price, asks ascending
  - a 6-level book → 6, not padded to 10
  - `summarise(None)` → `None`
  - (the sixth spec.md test — no `Level` anywhere carries `"fake"` — is
    better proven at the integration level in phase 4, once `run_fake_writer`
    is actually gone; see the note under Phase 4)
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — all five tests above pass, named as behaviour sentences
    per the testing convention, no `test_` prefix.
- Done looks like: `summarise()` compiles and is fully proven against hand-
  built `Book` values, with nothing yet calling it from a running task.
- Commit boundary: `src/merge.rs`, `src/lib.rs`. Reverting this phase (with
  phase 1 still in place) leaves the borrowed parser landed but unused by
  anything new — buildable on its own.

### Phase 3: Wire the pipeline — `src/aggregator.rs`, `src/server.rs`, `src/main.rs`

- Objective: The actual point of this step — connect the Binance feed to the
  gRPC server through a new aggregator task, deleting `run_fake_writer`.
  Necessarily one commit-sized unit rather than three, because the crate
  does not compile in any partial state between these three files: `main.rs`
  owns the channel construction that both `server.rs` and `aggregator.rs`
  depend on, so migrating the watch type or introducing the mpsc in only one
  of them leaves the other two referencing a type or a function that no
  longer exists.
- Main changes:
  - `src/aggregator.rs` (new): `VenueState { book: Book, last_update:
    Instant }`, `Aggregator { binance: Option<VenueState> }` (`Venue` itself
    is imported from `src/exchange/mod.rs`, defined in phase 1). An async
    task function that owns an `Aggregator` as a local variable, receives
    `(Venue, Book)` pairs off the bounded `mpsc::Receiver` (capacity 32, per
    decision 1), does a real `match venue { Venue::Binance => ... }` to
    update `self.binance` and call `merge::summarise(venue, Some(&book))`,
    wraps the result in `Arc::new`, and writes it into the
    `watch::Sender<Option<Arc<Summary>>>`. `recv()` returning `None` (feed's
    `Sender` dropped) ends the task's loop and the function returns — the
    same supervision shape step 2 already established, so `select!` in
    `main.rs` needs no new arm-handling logic, only a renamed task.
    `last_update` is written but not read this step (step 6 adds the check)
    — leave a comment saying so, matching spec.md's own framing, rather than
    leaving it looking like dead code with no explanation.
  - `src/server.rs`: `run_fake_writer` deleted entirely, including its
    doc comment's forward-reference to this step (update or remove it,
    don't leave a comment pointing at code that's now gone). `watch::Sender`/
    `Receiver<Option<Summary>>` becomes `Option<Arc<Summary>>` throughout —
    `AggregatorService.rx`, `router()`'s parameter. The `filter_map` in
    `book_summary` changes from `|opt| opt.map(Ok)` to something that clones
    the `Arc`'s contents into an owned `Summary` before handing it to
    `tonic` (e.g. `|opt| opt.map(|arc| Ok((*arc).clone()))`) — this is the
    line decision 5 is actually about: the deep clone still happens (`tonic`
    wants a `Summary` by value), it just happens after the `Arc::clone`
    leaves the watch's internal lock, not under it.
  - `src/main.rs`: the `(tx, rx) = watch::channel(None)` construction now
    types as `Option<Arc<Summary>>`. A new `mpsc::channel::<(Venue,
    Book)>(32)` is constructed alongside it. `run_feed`'s read loop, instead
    of only logging, sends each parsed `(Venue::Binance, Book)` pair down
    the mpsc `Sender` (a bounded send
    naturally applies backpressure to the feed loop if the aggregator falls
    behind — that's the point of decision 1, not a bug to work around).
    `fake_writer_handle` is replaced by an `aggregator_handle` spawning
    phase 3's aggregator task with the mpsc `Receiver` and the watch
    `Sender`. The `select!` keeps its existing three-arm shape (feed,
    aggregator, server) and existing error-propagation pattern — only the
    middle arm's task changes, not the supervision logic around it.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo run -- --pair ethbtc --port 50051`, then `grpcurl -plaintext
    127.0.0.1:50051 orderbook.OrderbookAggregator/BookSummary` (network
    permitting — report honestly if this environment can't reach Binance,
    per the project's standing honesty requirement from `004-grpc-server`'s
    plan) — confirm streamed `Level.exchange` reads `"binance"`, not
    `"fake"`, and that spread/levels look like real ETHBTC market data.
  - `grep -rn '"fake"' src/` returns nothing — the acceptance criteria's
    "no `\"fake\"` literal anywhere in `src/`" bullet, checked directly
    rather than inferred from the diff.
  - `docker compose up` — report actual observed behaviour in this
    environment (Docker daemon availability, Binance reachability through
    the configured proxy), not intended behaviour, per the same honesty
    standard `004-grpc-server`'s plan set.
- Done looks like: killing the feed (disconnect) still ends the whole
  process via the same three-task `select!` supervision step 2 established;
  `run_fake_writer` no longer exists anywhere in `src/`; a live `grpcurl`
  session shows real Binance data end to end.
- Commit boundary: `src/aggregator.rs`, `src/server.rs`, `src/main.rs`,
  `src/lib.rs` (adding `pub mod aggregator;`). Reverting this phase (with
  phases 1-2 still in place) restores step 2's fake-writer pipeline exactly,
  on top of an unused borrowed parser and an unused `summarise()` — both
  still independently buildable and tested.

### Phase 4: `tests/grpc.rs`

- Objective: Prove the real pipeline's stream contract, not the deleted
  fake writer's. The existing test constructs its own `watch::channel` and
  spawns `server::run_fake_writer(tx)` directly — since phase 3 deletes that
  function, this test's setup has to change structurally, not just its
  string assertion, even though spec.md's Tests section phrases it as a
  one-line assertion update.
- Main changes: `tests/grpc.rs` — replace the `run_fake_writer` spawn with a
  real (or near-real) drive through the actual pipeline: construct the mpsc
  channel from phase 3, spawn `aggregator`'s task on it, and send two
  distinct hand-built `Book` values down the mpsc `Sender` (real code path:
  `aggregator` → `summarise` → `watch`, not a mock of any of those) so the
  test still proves **two** distinct messages arrive, not one — preserving
  the existing test's core guarantee (see its doc comment: "reading exactly
  two consecutive messages is the one thing that actually exercises the
  `stream` contract"). Assertions change from `exchange == "fake"` to
  `exchange == "binance"`; the "no `Level` anywhere carries `\"fake\"`" test
  from spec.md's list is satisfied by this same assertion (asserting
  `== "binance"` on every level is equivalent to asserting `!= "fake"` given
  there is exactly one venue this step) — flagged here as an implementation
  choice, not a second, separate test, since spec.md lists them as two
  bullets but they resolve to one assertion once `run_fake_writer` no longer
  exists to test against.
- Verification:
  - `cargo test` green, this test's name and outcome reported explicitly
    (per this project's convention of not folding individual integration
    test results into an aggregate "tests pass" claim).
  - Confirm the test still does not depend on live network access — it
    drives the pipeline with hand-built `Book` values sent down the mpsc,
    not a real Binance connection, so it must remain runnable with no
    outbound network, same as today.
  - Inspection: still exactly two messages taken off the stream before any
    assertion, matching the existing test's documented reasoning.
- Done looks like: `cargo test` includes this test under its updated shape,
  it passes deterministically (no fixed port — unchanged from today), and it
  fails if `exchange` reverts to `"fake"` or if the aggregator collapses two
  distinct `Book` sends into one published `Summary` (it does not this step
  — dedupe is step 5 — so two sends must yield two distinct stream reads).
- Commit boundary: `tests/grpc.rs` alone. Reverting this phase (with phases
  1-3 still in place) leaves the pipeline shipped but its own integration
  test stale — not a safe end state, so this phase should not actually ship
  independently of phase 3 landing correctly first; noted for revert
  planning, not a recommendation to skip.

### Phase 5: `README.md`

- Objective: Describe what phases 1-4 actually shipped, once there's real
  end-to-end behaviour to document — per the standing rule from
  `specs/003-step-1-fixes/revisions.md` entry 1.
- Main changes: `README.md` — build-order table's step 3 row moves to
  "Done"; the gRPC section corrected to say the stream carries real Binance
  data (`exchange == "binance"`), not the step 2 placeholder; a short note
  on the fixed-point-vs-`f64` state of `Price`/`Amount` if not already
  covered (already documented as an `f64`-with-`total_cmp` decision per
  `src/model.rs`'s own doc comment and `specs/002-binance-feed/revisions.md`
  — this phase should not silently re-litigate that, only confirm the
  README still describes it accurately); `src/aggregator.rs` and
  `src/merge.rs` added to the Layout tree; the mpsc bound of 32 mentioned
  wherever backpressure/channel design is already discussed, since it's a
  design decision confirmed during spec review, not just an implementation
  detail.
- Verification:
  - Manually run every command the README's Quick Start / gRPC sections
    show and confirm actual output matches what's documented, to the extent
    this environment allows (same honesty note as phase 3).
  - Read-through: no leftover references to `run_fake_writer` or
    `"fake"`-labelled example output remain anywhere in the README.
- Done looks like: every claim in the updated sections matches what phases
  1-4 actually produced and actually verified in this environment.
- Commit boundary: `README.md` alone. Reverting it has no effect on build or
  test state.

## Cross-Cutting Considerations

- **`proto/orderbook.proto` untouched, checked every phase.** `git diff main
  --stat` at the tip of the branch must show zero diff for this file — none
  of this step's changes (borrowed parsing, `Arc<Summary>` internally, the
  mpsc) require or justify a schema change; `Arc` is purely an internal
  representation choice, invisible on the wire.
- **The six settled decisions in spec.md are not re-opened by planning.**
  The mpsc item type (`(Venue, Book)`) and `summarise()`'s `Venue` parameter
  are pinned above under "Plan Review Notes" — both chosen to give decision
  3's `Venue` enum a real compile-time match this step, not deferred to
  step 4.
- **`f64`-boundary discipline carries forward from step 1's revision.**
  `src/model.rs` already made the deliberate, documented choice to keep
  `Price`/`Amount` as `f64` newtypes rather than fixed-point (see that
  file's own doc comment and `specs/002-binance-feed/revisions.md`) — this
  is a standing decision this step's `summarise()` and `aggregator.rs`
  build on as given, not a rule this plan re-derives or second-guesses.
  `summarise()` converts to `f64` only at the `Summary`/`Level` boundary,
  consistent with that existing pattern.
- **Untouched-files discipline.** No phase in this plan should need to
  touch `src/config.rs`, `src/proxy.rs`, `src/telemetry.rs`, or
  `Dockerfile`/`compose.yml` — this step is feed-to-gRPC wiring only. A
  phase whose diff unexpectedly touches one of these is a stop-and-flag
  condition, matching `004-grpc-server`'s plan's own convention for its
  three off-limits files.
- **Scope discipline.** No phase adds the Bitstamp feed, a real two-book
  `merge()`, reconnection/staleness enforcement (only the unused
  `last_update` field), or publish-dedupe — all explicitly OUT per spec.md,
  landing in steps 4-6.

## Verification Gates

Before this branch is considered ready to hand off:

- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all clean at the tip of the branch.
- `cargo test` reports at least the six tests spec.md names (five in
  `src/merge.rs`, one — possibly folded per the phase 4 note above — in
  `tests/grpc.rs`), each identifiable by a behaviour-sentence name, not
  folded silently into an aggregate pass/fail count.
- `grep -rn '"fake"' src/` returns nothing.
- `grpcurl -plaintext 127.0.0.1:50051 orderbook.OrderbookAggregator/BookSummary`
  against a live `cargo run` streams real data with `exchange == "binance"`
  — actual output quoted, not assumed, network permitting (report honestly
  if this environment can't reach Binance).
- `docker compose up` through the configured proxy is either genuinely
  exercised with real output reported, or its environment limitation is
  stated plainly — same non-negotiable honesty requirement
  `004-grpc-server`'s plan set for this project.
- `git diff main --stat` at the tip of the branch shows zero diff for
  `proto/orderbook.proto`, `src/config.rs`, `src/proxy.rs`,
  `src/telemetry.rs`.
- README.md's claims match what was actually shipped and actually verified
  in this environment, not what was planned.

## Expected Drift Triggers

If any of the following becomes true while implementing, update `spec.md`
before continuing rather than improvising past it:

- Phase 1 discovers that `serde` cannot actually borrow through
  `Price::parse`/`Amount::parse` as expected (e.g. an escape sequence in a
  price string forces an owned `String` somewhere in the chain) — decision
  4 already says to report this rather than silently falling back to owned
  data; that report belongs in a spec.md update, not a silent phase-1 scope
  change.
- Phase 3 discovers the bounded `mpsc::channel(32)` genuinely fills up
  under a live Binance connection (the aggregator can't keep pace with
  `depth20@100ms`) — this would be a real signal about the capacity choice
  decision 1 settled during spec review, worth surfacing rather than
  quietly raising the bound to make the symptom disappear.
- Phase 3's `Arc<Summary>`-in-`watch` change turns out not to compose
  cleanly with `WatchStream`'s existing `filter_map` adapter (e.g. the
  closure's ownership doesn't type-check the way decision 5 assumed) — a
  structural surprise worth flagging, not a reason to revert to
  `Option<Summary>` and quietly drop the "clone moves outside the lock"
  guarantee decision 5 was written to give.
- Phase 4 finds that folding spec.md's two "fake"-related tests into one
  assertion (per this plan's note under Phase 4) loses real coverage — e.g.
  a plausible bug exists that the `exchange == "binance"` assertion alone
  wouldn't catch but a separate "no exchange field anywhere equals `fake`"
  scan would — that's worth a second, explicit test, not a silent
  under-implementation of spec.md's Tests list.
- `docker compose up` in phase 3/5's verification cannot be run at all in
  this environment (no Docker daemon, no route to Binance even through the
  proxy) — report this as "not verified here," not silently omitted.

## Plan Review Notes — resolved before task-writer

Two structural details spec.md's six decisions left unstated were resolved
during plan review (not a spec change, since neither contradicts a settled
decision — both just pick a concrete shape decision 3 already implied):

- **`Venue` needs a real runtime use this step, not just a forward
  declaration.** Decision 3's own stated reason — "when Bitstamp arrives,
  every place that needs updating stops compiling" — only holds if `Venue`
  is actually matched on somewhere in this step's code. So: the mpsc item
  type is `(Venue, Book)`, not `Book` alone, and the aggregator task does a
  real `match venue { Venue::Binance => ... }` when updating its state.
  `enum Venue { Binance }` (plus a `Display`/label impl yielding `"binance"`)
  is defined in `src/exchange/mod.rs` in phase 1, since it's an
  exchange-domain type and phase 1 is already touching that module.
- **`summarise()` takes `Venue`, sourcing the `exchange` label from it.**
  `pub fn summarise(venue: Venue, book: Option<&Book>) -> Option<Summary>` —
  every `Level` in the result gets `exchange: venue.to_string()` (or
  equivalent), not a hardcoded `"binance"` literal in `merge.rs`. This keeps
  `merge.rs` exchange-agnostic (consistent with decision 6's framing) and
  gives `Venue` its compile-time teeth immediately rather than at step 4.
  Phase 2's tests construct a `Venue::Binance` fixture value alongside their
  hand-built `Book`s.

Phase 1's scope now includes `src/exchange/mod.rs`'s `Venue` enum
(previously only implied to belong somewhere in phase 3); phase 2's
`summarise()` signature and phase 3's mpsc item type above supersede the
earlier "left to the implementer" framing for these two points.
