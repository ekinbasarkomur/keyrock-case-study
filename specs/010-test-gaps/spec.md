---
spec_name: "Step 8 — the tests the earlier steps didn't cover"
spec_id: "010"
spec_folder: "010-test-gaps"
status: "approved"
created_at: "2026-08-26"
updated_at: "2026-08-26"
created_by: "spec-synthesizer"
creation_mode: "human-brief"
source_inputs:
  - "inputs/001-step-8-brief.md"
source_agents: []
goal: "Eight targeted tests that cover composition and wiring gaps steps 0-7 left behind — the interactions between already-tested mechanisms, not new mechanisms or new production behavior."
purpose: "The 42 existing tests cover each mechanism (backoff, the token bucket, the staleness filter, the merge) in isolation, but nothing exercises the loops that wire those mechanisms together — the run_feed loop that uses backoff and the bucket together, and the aggregator loop that applies the staleness filter and publishes the result. What breaks in a refactor is usually the wiring (a call moved outside a loop, a clone that stops being a clone), and none of that shows up in a unit test of one piece alone."
parent_request: "specs/010-test-gaps/inputs/001-step-8-brief.md (human brief, step 8 of the project's build order)"
related_paths:
  - "src/feed.rs"
  - "src/aggregator.rs"
  - "src/model.rs"
  - "src/exchange/mod.rs"
  - "src/exchange/binance.rs"
  - "src/exchange/bitstamp.rs"
  - "tests/grpc.rs"
  - "tests/feed.rs"
  - "README.md"
verification_level: "mixed"
complexity: "small"
---

# Spec: 010-test-gaps

## Problem

Steps 0-7 landed 42 tests, each naming a bug it catches — no coverage
padding. But the tests cover mechanisms, not composition: backoff is tested
in isolation, the token bucket is tested in isolation, the staleness filter
is tested in isolation. The loop in `run_feed` that actually uses backoff
and the bucket together is untested, and so is the aggregator loop that
actually applies the staleness filter and publishes the result. A refactor
that silently breaks the wiring between two already-correct pieces — a
`subscribe_message` call that moves outside the reconnect loop, a clone that
stops being a clone — passes every existing test and only shows up live,
against a real exchange.

Two further gaps are behavior questions, not just coverage gaps: what
happens today when one level in a book fails to parse, and what happens
today when a price string is negative. Both were investigated (not
guessed) before this spec was written — see Design, tests 5 and 6.

## Goal

Eight new tests land, each naming the bug it catches. No new production
mechanism is introduced — this step tests interactions that already exist.
`src/merge.rs` shows zero diff, confirmed by `git diff main --stat --
src/merge.rs`.

## Purpose

The mechanisms are the easy part to get right and the easy part to test.
The wiring between them is where a refactor actually breaks something, and
that risk is invisible to a unit test of one piece in isolation. This step
closes that specific gap rather than chasing a coverage percentage.

## Out of Scope

- **`src/merge.rs` must not change** — no test in this packet touches merge
  logic; it was already covered by step 5's 8-test list and nothing here
  revisits it.
- No coverage tooling, no coverage percentage target. The bar stays "name
  the bug it catches" — a percentage target invites tests for lines nobody
  would ever break.
- No mock websocket framework. Test 3's local `TcpListener` is the entire
  harness needed; nothing more general gets built.
- No property-based testing. Worth doing eventually, not worth the time
  this step has, which is better spent on step 9's latency numbers.
- No new production code, with one narrow exception: whatever (if anything)
  tests 5 and 6 turn out to need once their current behavior is confirmed.
  Both were investigated before this spec was written (see Design) and both
  concluded "no change needed" — so in practice this step is test-code-only.
  The one non-code addition is a `README.md` production-notes line
  documenting the cost of test 6's accept decision (see Design, test 6) —
  a doc change, not a behavior change.

## Current State

Verified by reading the current source, not assumed:

- `src/feed.rs`'s `run_feed<E: Exchange>` is `pub`, generic over `Exchange`,
  and (since step 7 / `009-resilience`) never returns on a closed or failed
  connection — it reconnects forever via a private `Backoff` and a private
  per-venue `TokenBucket`, both already unit-tested in isolation. The
  subscribe call (`if let Some(msg) = exchange.subscribe_message(pair) {
  ws.send(...) }`) sits inside `run_once_inner`, which is called fresh on
  every reconnect attempt from `run_feed`'s outer loop — i.e. the subscribe
  call already re-fires on every reconnect. This was the exact thing
  `009-resilience`'s spec asked to be *confirmed*, not assumed, during
  implementation; test 1 is that confirmation made permanent.
- `src/aggregator.rs`'s `pub async fn run(rx, tx, pair)` owns per-venue
  state (`BTreeMap<Venue, VenueState>`), filters stale venues via the
  private `fresh_venues(venues, now)` (already unit-tested with a hand-built
  `now`), and publishes into a `watch::Sender<Option<Arc<Summary>>>` when
  `merge::merge` returns `Some`. `fresh_venues` already takes `now` as a
  parameter rather than calling `Instant::now()` internally, which is what
  makes it possible to drive the staleness-narrows-the-publish path with a
  fixed, hand-computed clock rather than `tokio::time::pause` (not
  available — the `tokio` dependency here uses the `full` feature, which
  does not include `test-util`).
- `src/model.rs`'s `Price::parse`/`Amount::parse` are `s.parse::<f64>().ok()
  .map(Self)` — no sign check. A negative price string parses successfully
  today.
- `src/exchange/binance.rs` and `src/exchange/bitstamp.rs` each have their
  own private `parse_levels(levels: &[[&str; 2]]) -> Option<Vec<(Price,
  Amount)>>`, both shaped as `.iter().map(|[price, amount]| { ...?...
  }).collect()`. `Iterator::collect()` into `Option<Vec<_>>` short-circuits
  on the first `None` produced by the closure.
- `src/exchange/mod.rs`'s `Venue` implements `Display` (`"binance"`,
  `"bitstamp"`, lowercase) — this string is what ends up in the wire
  `Level.exchange` field. Nothing currently asserts the exact casing.
- `tests/grpc.rs` already builds a real `server::router(rx)`, serves it on
  an OS-assigned port, and connects a real `tonic` client — the existing
  test uses exactly one client. `OrderbookAggregatorClient::connect` can be
  called again against the same address to get an independent second
  client/connection.

## Proposed Design

Eight tests, **in priority order — if time runs short, the first four are
the ones that matter.** Every test names the bug it catches, per this
project's standing testing convention: a test with no nameable bug gets
dropped, not written for coverage's sake.

**Filing follows the project's access-based rule**: a test that only needs
public items belongs in `tests/` as an integration test against the real
binary/library surface; a test that needs a private item (an internal
struct, a private helper function) belongs in that file's own `#[cfg(test)]
mod tests`, next to the code it reaches into.

### 1. Re-subscribe on every reconnect

**The most dangerous gap in the codebase.** Bitstamp's subscription is
per-connection. If a future refactor moves the `subscribe_message` call
outside the reconnect loop, the socket opens, no error appears anywhere,
and no data ever arrives again — and staleness would actively hide the
cause, by just marking the venue dead rather than pointing at a missing
subscribe. `009-resilience`'s spec asked for this to be "confirmed during
implementation" rather than assumed; a test is a better confirmation than a
one-time reading of the code, because it stays true across every future
change.

**Test**: drive `run_feed` against a controllable local server (a fake
`Exchange` whose `subscribe_message` increments a shared counter, connected
to a local `TcpListener`-backed socket) through two forced disconnects, and
assert the counter reads exactly three (initial connect + two reconnects).

**Filing**: `run_feed` and the `Exchange` trait are both `pub` already —
this test needs no private item, so it's an integration test. New file
`tests/feed.rs`, since neither `tests/cli.rs` nor `tests/grpc.rs` currently
owns a local-socket feed harness and test 3 below needs the identical
harness.

### 2. Two clients, and one of them leaving

`watch` over `broadcast` is the decision most likely to come up under
questioning, and its entire justification is fan-out to N subscribers — yet
every existing test exercises it with N=1. A `BookSummary` implementation
that moved the `Receiver` instead of cloning it, or handed every caller the
same one, would work perfectly with one client and break silently with two.
Nothing today catches that.

**Test**: connect two independent `OrderbookAggregatorClient`s against the
same `server::router(rx)` instance, assert both receive a `Summary`, drop
one client's stream, and assert the other keeps streaming (a second publish
still reaches it).

**Filing**: only the public `tonic` client surface and `server::router` are
needed — integration test, appended to the existing `tests/grpc.rs` (which
already owns the real-server-plus-real-client harness this needs).

### 3. The read loop retries rather than returning

Reconnection's entire premise is that `run_feed` never returns. Backoff's
delay numbers are unit-tested; the loop that actually consumes them on a
real socket close is not.

**Test**: bind a local `TcpListener`, accept a connection and immediately
close it, and assert `run_feed` comes back for another connection attempt
(a second `accept()` on the same listener succeeds) — **and time the gap
between the two `accept()` calls, asserting it's at least the first
backoff delay.** Proving the loop retries at all is not enough: a loop that
reconnected instantly in a tight spin would pass a retry-only assertion and
would be a *worse* bug than not reconnecting — it's exactly the rate-limit
failure pieces 3 and 4 of `009-resilience` (the stability-gated reset, the
token bucket) exist to prevent. This makes the assertion "it retries after
actually waiting," not just "it retries." Keep the bound one-sided (at
least the first delay, no upper bound) so jitter and scheduler variance
can't make the test flaky.

Deliberately not done by abstracting `connect()` behind the `Exchange`
trait to make it mockable — the trait describes protocol *data* (a URL, a
subscribe message, a parse function), not control flow, and adding a
`connect()` method to make it swappable would undo that separation (the
same reasoning `009-resilience` and the sync-trait design already
established). A local listener is less invasive and exercises real socket
behavior instead of a fake.

**Filing**: `run_feed` and `Exchange` are `pub` — integration test,
`tests/feed.rs`, alongside test 1 (same local-listener harness style).

### 4. A stale venue actually narrows the published Summary

The staleness filter (`fresh_venues`) is tested in isolation. What isn't
tested is that a venue going quiet changes what actually comes out the
other end — the published `Summary` itself.

**Test**: send a book from both venues, advance a hand-computed `now` past
Binance's staleness threshold (1.5s) while staying under Bitstamp's (8s),
send a fresh book from Bitstamp only, and assert the resulting `Summary`
(built by feeding the same `fresh_venues` + `merge::merge` sequence
`aggregator::run`'s loop body performs) carries only Bitstamp's levels.

**Watch out for the deadlock — this is a concrete implementation warning,
not a hypothetical.** `watch` only ever holds the latest value: a sender
that batches two `.send()` calls before the receiver's first read collapses
both updates into one observable value, and a test that then tries to read
*two* distinct values off that receiver hangs forever on the second read.
This already happened for real during `005-aggregator`'s implementation
(`specs/005-aggregator/revisions.md` entry 3) and again shaped
`tests/grpc.rs`'s existing test — it is not a flaky-test risk to wave off
here. This test must interleave every send with a read (send, read, advance
the clock, send, read), never batch sends up front.

**Filing**: this needs `fresh_venues`, `VenueState`, and `Aggregator`'s
private internals — `aggregator::run` itself does not take an injectable
clock (it calls `Instant::now()` internally), and `tokio::time::pause`
isn't available (this project's `tokio` dependency uses the `full` feature
without `test-util`). Unit test in `src/aggregator.rs`'s own `mod tests`,
following the same "pass `now` in by hand" pattern the file's existing
`stale_venue_excluded_from_merge` / `fresh_venue_included` tests already
use, but driving a real `watch::channel` through it end to end (rather than
only asserting on `fresh_venues`'s return value) so the test proves the
*publish* narrows, not just the filter.

### 5. A level that won't parse — investigated, not guessed

**What the code does today, confirmed by reading `src/exchange/binance.rs`
and `src/exchange/bitstamp.rs`'s `parse_levels` functions**: both are
`.iter().map(|[price, amount]| { let price = Price::parse(price)?; let
amount = Amount::parse(amount)?; Some((price, amount)) }).collect()`.
`Iterator::collect()` into `Option<Vec<_>>` short-circuits on the first
`None` the closure produces — so if *any* single level in a book fails to
parse, `parse_levels` returns `None`, which propagates via `?` in the
caller's `parse()`, and the *entire* message is rejected. This already
matches the brief's own stated preference: "reject the whole message… a
missing tick is honest and staleness picks up the slack," rather than
silently publishing a book with a hole in it (the dangerous outcome the
brief specifically flagged — a silently short book with a gap in the top
ten and nothing in the logs).

**No production code change is needed.** The test locks in already-correct
behavior rather than fixing a bug.

**Test**: feed `parse()` a book containing one malformed level (e.g. a
non-numeric price string in one bid), and assert the result is `None` — the
whole message rejected, not a 19-level book.

**Filing**: needs the private `parse_levels` boundary behavior exercised
through each exchange's own public `parse()` — unit test in each of
`src/exchange/binance.rs`'s and `src/exchange/bitstamp.rs`'s `mod tests`.
One test per exchange, not one shared test, because each file implements
its own independent `parse_levels` (the bitstamp.rs comment on the function
already calls it "a direct copy" of binance.rs's shape, not a shared
implementation) — a future refactor could regress one venue's copy without
touching the other's, and a single test filed against only one exchange
would miss that.

### 6. A negative price — investigated, decided, not guessed

**What the code does today, confirmed by reading `src/model.rs`**:
`Price::parse`/`Amount::parse` are `s.parse::<f64>().ok().map(Self)` — no
sign validation at all. A negative price string parses successfully,
silently, today: not rejected, not a panic.

**Decision (asked, not assumed): accept, and document as intentional.**
A negative price and a negative spread are not the same kind of thing, and
the crossed-book precedent does not actually justify this on its own — a
negative spread is a *computed result* from two individually valid prices,
a real market state (two venues with no shared matching engine, one's best
ask below the other's best bid); it's information, and clamping it would
destroy something true. A negative price is an *input*. No real market
state produces one — it isn't information about the market, it's
corruption. Rejecting the input while publishing the computed result is
therefore consistent, not the inconsistent halfway measure it might look
like at first glance.

The actual reason to accept it: this parsing layer reads wire data and
reports what the venue sent — it does not enforce domain rules. Validation
against domain rules (price must be positive) belongs a layer above the
parser, and no such layer exists in this codebase today. That absence is a
deliberate scope decision for this step, not a consequence of the
crossed-book design elsewhere — the two are unrelated design points that
happen to both involve a sign.

**What accepting it actually costs, stated plainly rather than left
implicit:** a corrupted frame carrying a negative price sorts to the front
of the book (it looks like the best available price) and propagates all
the way to gRPC clients. Staleness does not catch this — the venue is
actively publishing, just publishing nonsense, so nothing about its
last-update timestamp looks wrong. "Accept" concretely means "a corrupted
frame reaches clients unfiltered." This limitation must be added to the
README's production-notes section as its own line, not left implicit:
`The parser doesn't validate price signs — it reports what the venue sent.
A corrupted frame carrying a negative price would sort to the front of the
book and propagate. Input validation belongs in a layer above the parser
and isn't built here.`

**No production code change is needed.**

**Test**: `Price::parse("-0.001")` returns `Some(Price)`, not `None` — locks
in "a negative price parses successfully" as documented, expected behavior,
not an oversight.

**Filing**: unit test in `src/model.rs`'s `mod tests`, next to the existing
`Price`/`Amount` round-trip and ordering tests.

### 7. `Venue::Display` is a wire contract

`Display` produces `"binance"`/`"bitstamp"`, and that exact string is what
ends up in the wire `Level.exchange` field — the brief's own example output
shows it lowercase. Nothing today asserts the casing. A `Display` change
(e.g. `"Binance"`) would compile cleanly, every one of the 42 existing
tests would still pass, and every client downstream would silently start
receiving a different string.

**Test**: two assertions — `Venue::Binance.to_string() == "binance"` and
`Venue::Bitstamp.to_string() == "bitstamp"`.

**Filing**: `Venue` and its `Display` impl are both `pub`, but this is
small, internal, and belongs next to the type it's about — unit test in
`src/exchange/mod.rs`'s existing `mod tests`, alongside
`thresholds_differ_per_venue`.

### 8. An empty side parses without panicking

`"bids": []` is legal JSON and a plausible message from a venue in a
strange state (e.g. a temporarily one-sided book). `merge()` already
handles an empty side (step 5's test list includes `one_venue_still_merges`
et al.), but the parser itself has never been exercised against an empty
side — an off-by-one or an `.unwrap()` on `.first()` in a future change to
`parse_levels` or its caller would go unnoticed until it hit a real feed.

**Test**: feed each exchange's `parse()` a payload whose `bids` (or `asks`)
array is `[]`, and assert it returns `Some(Book)` with an empty `bids`
(`asks`) vec, not `None` and not a panic.

**Filing**: one test per exchange, same reasoning as test 5 — each has its
own independent `parse_levels`/`parse` — unit test in each of
`src/exchange/binance.rs`'s and `src/exchange/bitstamp.rs`'s `mod tests`.

## Acceptance Criteria

- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all pass.
- `git diff main --stat -- src/merge.rs` shows no diff.
- No production code changes land from this packet except whatever tests 5
  and 6 are found to actually need — and per the investigation in Design,
  the expected answer is none.
- For tests 5 and 6, the current behavior is reported (in this spec, above)
  before either test was written — not discovered after the fact.
- All eight tests exist, each with a doc comment or inline comment naming
  the bug it catches, matching the style of the existing 42.
- The binding number is eight numbered gaps closed, not a specific test
  count — tests 5 and 8 are each filed per-exchange (see Design), so the
  actual count of new `#[test]` functions is naturally higher than eight.

## Invariants and Critical Don'ts

- `src/merge.rs` does not change. This is the scope check's centerpiece.
- No coverage tooling, no coverage percentage gate, no mock websocket
  framework beyond test 1/3's local `TcpListener`, no property-based
  testing — all explicitly out of scope per the brief.
- Tests 5 and 6 do not silently "fix" the behavior they're testing — both
  were investigated and a deliberate decision made (reject-the-whole-
  message for 5, accept-and-document for 6) before any test was written.
- Every test names a real bug in its own comment. A test with no nameable
  bug is dropped, not written for coverage.
- Test 4 interleaves every send with a read against the `watch` channel —
  never batches sends up front. See test 4's Design section for the
  concrete deadlock this avoids.

## Risks and Tradeoffs

- Tests 1 and 3 both drive `run_feed` against a local `TcpListener` rather
  than a real exchange — this proves the reconnect *loop's* wiring
  (subscribe re-fires, the loop retries) but not real-exchange-specific
  behavior (TLS handshake quirks, real close-frame codes). That's already
  covered by this project's existing real-fixture parser tests and by
  `009-resilience`'s live-proxy-interruption acceptance criteria; this
  packet's job is the loop wiring, not exchange fidelity.
- Test 4's clock is hand-advanced (a `now` value computed by hand, not
  `tokio::time::pause`) because `test-util` isn't enabled on the `tokio`
  dependency. Enabling `test-util` was considered and rejected here to
  avoid a dependency-feature change for one test, consistent with this
  project's pattern of treating dependency-feature changes as their own
  deliberate, confirmed decision (see the `rustls`/`tls12` precedent) — the
  existing "pass `now` as a parameter" pattern in `src/aggregator.rs`
  already supports the hand-advanced approach without it.
- Splitting tests 5 and 8 into one-per-exchange (rather than one shared
  test) roughly doubles their count relative to a literal reading of "eight
  tests" — justified in Design by each exchange owning an independent
  `parse_levels` copy that could regress independently, but worth flagging
  as an interpretation choice rather than a literal instruction from the
  brief.

## Testing Strategy

Required real verification (all eight land as real tests, not descriptions
of tests):

- Test 1 — `tests/feed.rs`: real `run_feed` against a real local TCP
  listener, forcing two disconnects, asserting exactly three
  `subscribe_message` calls. Regression coverage for the single most
  dangerous silent-failure mode in the reconnect path (a moved subscribe
  call).
- Test 2 — `tests/grpc.rs`: real `tonic` clients (two, not one) against a
  real `server::router`. Regression coverage for the `watch`-fan-out
  contract this project's architecture is built on.
- Test 3 — `tests/feed.rs`: real local TCP listener, accept-then-close,
  asserting `run_feed` reconnects rather than returning, and that the gap
  before the second `accept()` is at least the first backoff delay.
  Regression coverage for reconnection's core premise *and* for a
  tight-spin reconnect loop, which would be worse than not reconnecting.
- Test 4 — `src/aggregator.rs` unit test: real `merge::merge` and a real
  `watch::channel`, driven with a hand-advanced clock, sends interleaved
  with reads. Regression coverage for staleness actually reaching the
  published output, not just the internal filter.
- Test 5 — `src/exchange/binance.rs` and `src/exchange/bitstamp.rs` unit
  tests: real `parse()` against a hand-built malformed-level payload per
  exchange. Regression/behavior-lock coverage for "reject the whole
  message," confirmed correct before the test was written.
- Test 6 — `src/model.rs` unit test: real `Price::parse` against a negative
  literal. Behavior-lock coverage for the accept-and-document decision.
- Test 7 — `src/exchange/mod.rs` unit test: `Venue::Display` output.
  Regression coverage for the wire-contract string.
- Test 8 — `src/exchange/binance.rs` and `src/exchange/bitstamp.rs` unit
  tests: real `parse()` against an empty-side payload per exchange.
  Regression coverage for a panic/`.unwrap()` on an empty side.

Optional supporting checks:

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` (already
  listed under Acceptance Criteria — repeated here as the fast, cheap check
  to run before the full suite).

## Rollback Plan

Each of the eight tests is independent, additive, test-code-only (per the
investigation in Design, tests 5 and 6 need no production change either) —
any one can be reverted individually with no effect on the others or on
production behavior. No feature flag or staged rollout is needed; this is
not a change to shipped behavior.

## Open Questions

None remaining. The brief flagged two items as needing an answer before
implementation — what a malformed level does today (test 5) and what a
negative price does today (test 6) — and both were investigated by reading
`src/exchange/binance.rs`, `src/exchange/bitstamp.rs`, and `src/model.rs`
before this spec was written, with the negative-price question additionally
requiring (and receiving) a direct decision from the user: accept and
document, not reject. Both are recorded as resolved facts in Design, tests
5 and 6, above, with the evidence that resolved them — this is the pattern
`009-resilience`'s spec used for its own measured decisions (Bitstamp's
staleness threshold, the grace period), applied here to a behavior
investigation rather than a live measurement.
