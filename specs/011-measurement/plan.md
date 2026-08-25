# Plan: 011-measurement

## Summary

Seven phases, matching spec.md's own "Order" section exactly — like
`009-resilience`, this packet's sequencing is not a planning judgment call,
it's a direct copy of an ordering spec.md already fixed (the prediction has
to exist before instrumentation, the release-profile change is deliberately
allowed to land ahead of the dedup *decision* while still being measured
before/after itself, and the 24-hour run is explicitly last and explicitly
allowed to outlive this packet's own merge).

- Phase 1 — the prediction, already written into spec.md; this phase is a
  single-line confirmation that it landed as its own commit, not new work.
- Phase 2 — Piece 1: latency instrumentation (three histograms: parse,
  merge+publish, total) plus Piece 2's dedup-rate *counting* only — no skip
  yet (`src/model.rs`, `src/exchange/binance.rs`, `src/exchange/bitstamp.rs`,
  `src/aggregator.rs`, `Cargo.toml`).
- Phase 3 — Piece 3: release profile (`lto = "fat"`, `codegen-units = 1`),
  measured before/after against the Phase 2 build (`Cargo.toml`).
- Phase 4 — Piece 2's decision: implement the dedup skip only if the
  measured rate clears ~30%, either way record the number
  (`src/aggregator.rs`).
- Phase 5 — Piece 4: the load test binary and the 100/500/1000-subscriber
  runs (`src/bin/loadtest.rs`, `Cargo.toml` if a new dependency is needed).
- Phase 6 — README: prediction vs. result, every measured number, the
  updated production notes (`README.md`).
- Phase 7 — Piece 5: start the 24-hour run against the shipped build from
  Phases 1-6; its write-up lands as a follow-up if it outlives this
  packet's own review.

The one structural invariant that spans every phase from Phase 2 onward,
checked explicitly at each one rather than assumed to hold by the end:
**`git diff main --stat -- src/merge.rs` must show no diff.** This is
spec.md's own stated "single most load-bearing invariant in this packet"
and its own Acceptance Criteria line — every timestamp, every histogram
recording, and the dedup comparison live in `src/aggregator.rs` and
`src/exchange/*.rs`, wrapped around the existing `merge::merge()` call, not
inside it. The check runs after Phase 2 (first phase that touches
`src/aggregator.rs`), Phase 4 (the phase under the most pressure to reach
into `merge()`, since dedup compares merge's own output), and again at the
tip in Phase 6 — matching the pattern `009-resilience/plan.md` and
`007-merge`'s scope check both already used for this exact file.

A second, related check runs alongside it whenever `src/model.rs` is
touched (Phase 2 only): `merge()`'s signature and body must show zero diff
even though `Book` itself grows two new `Instant` fields — the two new
timestamps are written by `Exchange::parse` and read by
`src/aggregator.rs`, never by `src/merge.rs`, which never receives a
`Book` with a clock reachable from it in the first place (it receives
`&BTreeMap<Venue, &Book>`, and nothing in its own code path touches the new
fields).

## Phase Breakdown

### Phase 1: the prediction is already committed — confirm, don't redo

- Objective: spec.md's Order item 1 ("this spec, prediction included —
  first commit, alone") already happened; this phase's only job is to
  confirm that commit exists on this branch before Phase 2 starts, so a
  later reader can see the prediction predates any instrumentation, per the
  brief's explicit requirement.
- Main changes: none. No source file changes in this phase.
- Verification:
  - `git log --oneline specs/011-measurement/spec.md` — confirm the
    spec (with its Prediction section) has its own commit, and that no
    later commit on this branch edits the Prediction section's numbers
    after the fact.
- Done looks like: the prediction's commit is confirmed to exist and to
  predate Phase 2's first commit — the evidence that "written down before
  any instrumentation exists" is actually true, not just claimed.
- Commit boundary: none — this phase produces no commit of its own.

### Phase 2: latency instrumentation + dedup-rate counting — `src/model.rs`, `src/exchange/binance.rs`, `src/exchange/bitstamp.rs`, `src/aggregator.rs`, `Cargo.toml`

- Objective: land Piece 1 (three histograms, split by span, logged every
  ~30s) and Piece 2's *measurement only* (a duplicate-rate counter, no send
  skipped yet) — the largest phase in this packet, and the one with the
  strongest pull toward accidentally reaching into `src/merge.rs`.
- Main changes:
  - `Cargo.toml`: move `hdrhistogram` from `[dev-dependencies]` to
    `[dependencies]`.
  - `src/model.rs`: add `parse_started_at: Instant` and `parsed_at: Instant`
    to `Book`. Confirm during implementation whether this requires relaxing
    `Book`'s current `#[derive(PartialEq, Eq)]` (an `Instant` doesn't derive
    `Eq` meaningfully for test-fixture comparison the way the existing
    `model.rs` unit tests use it) — if so, resolve by hand-implementing the
    comparison the existing tests need rather than silently dropping a
    derive spec.md didn't ask to change.
  - `src/exchange/binance.rs`, `src/exchange/bitstamp.rs`: each `parse`
    stamps `parse_started_at` via `Instant::now()` at the top, before
    `serde_json::from_str` runs, and stamps `parsed_at` immediately before
    returning `Some(Book { .. })` — both stamps only on the success path;
    a `None` return (non-book message) records neither, matching the
    existing "not every message is a book" contract these functions
    already have.
  - `src/aggregator.rs`: `Aggregator` gains three `hdrhistogram::Histogram`
    fields (parse span, merge+publish span, total), an update counter, and
    a duplicate counter, plus `last_published: Option<Arc<Summary>>` (Piece
    2's precondition, wired in this phase even though the skip itself is
    Phase 4's job). The `run()` loop, around the existing
    `fresh_venues`/`merge::merge` call: records the parse-span sample as
    soon as a book is received (independent of whether it's ultimately
    published), records the merge+publish span and reads a fresh
    `Instant::now()` for the total span only after `merge::merge()` returns
    `Some` and the `tx.send` actually happens, and compares the resulting
    `Summary` against `last_published` to increment the duplicate counter
    (comparison only in this phase — `last_published` updates on every
    merge, and no send is skipped yet). A `tokio::time::interval` (~30s)
    logs p50/p99/p99.9 for all three histograms, the sustained update rate,
    and the running duplicate percentage.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — new unit tests in `src/aggregator.rs`'s `mod tests`,
    following the file's existing "pass the clock/timestamps in by hand"
    pattern (`past_grace`, `fresh_venues`) rather than calling
    `Instant::now()` inside the function under test where avoidable:
    - a `Book` with known `parse_started_at`/`parsed_at` values produces a
      recorded sample in both the parse histogram and the merge+publish
      histogram once merged and published.
    - a book filtered out as stale (reusing the existing
      `stale_venue_excluded_from_merge` fixture shape) contributes to
      neither the merge+publish histogram nor the total.
    - two structurally identical merged `Summary`s (same venues, same
      books) compare equal via the derived `PartialEq` — locking in the
      precondition the duplicate counter depends on, not restating
      `prost`'s own derive.
    - a changed book (one different price) produces a merged `Summary`
      that compares unequal to the previous one.
  - Existing 32-test baseline still passes unedited (same standard every
    prior packet's plan in this project has held Phase-1-equivalent work
    to).
  - **`git diff main --stat -- src/merge.rs` — zero diff, checkpoint 1 of
    3.** This is the phase where the temptation to pass a timestamp into
    `merge()` or `merge_side()` (since the spans wrap directly around the
    `merge::merge()` call) is highest — any nonzero diff here is a
    stop-and-flag condition, not a design call to make unilaterally.
  - Manual check: `cargo run -- --pair ethbtc --port 50051` against real
    (or proxied) connectivity for a couple of minutes, `RUST_LOG=info`,
    confirm the periodic log line actually appears roughly every 30s with
    non-degenerate p50/p99/p99.9 numbers (not all zero, not all identical —
    either would indicate the histogram isn't actually being fed real
    samples).
  - `git diff main --stat` overall — confirm only `src/model.rs`,
    `src/exchange/binance.rs`, `src/exchange/bitstamp.rs`,
    `src/aggregator.rs`, `Cargo.toml` (plus `specs/011-measurement/`)
    touched.
- Done looks like: a real periodic log line with three genuinely distinct
  histograms fed from live data, a duplicate-rate counter that measures but
  does not yet act, and `src/merge.rs` showing zero diff.
- Commit boundary: this phase's changes as one commit (Piece 1 +
  Piece 2's measurement-only counter travel together, since the counter
  depends on `last_published` state introduced alongside the histograms).
  Reverting it returns to the pre-instrumentation aggregator with no
  latency or dedup visibility — a real regression against this packet's
  goal, but a buildable, functioning state.

### Phase 3: release profile, measured before and after — `Cargo.toml`

- Objective: add `lto = "fat"` and `codegen-units = 1` to
  `[profile.release]`, with Phase 2's histogram used to produce a real
  before/after p50 comparison rather than assuming the flags help.
- Main changes:
  - `Cargo.toml`: `[profile.release]` gains `lto = "fat"` and
    `codegen-units = 1` alongside the existing `strip = true`. No
    `panic = "abort"` — spec.md's Out of Scope explicitly excludes it (it
    would collapse `JoinError::is_panic()`'s panic-vs-cancellation
    distinction in `src/main.rs`'s `JoinSet` supervisor).
- Verification:
  - Build and run the release binary **before** this change (`git stash`
    the `Cargo.toml` edit or check out the Phase-2 tip), against a fixed
    local workload (real Binance + Bitstamp connections for a fixed
    window, e.g. 5 minutes), record `cargo build --release`'s wall time and
    Phase 2's logged p50 for the total span at the end of the window.
  - Apply the `Cargo.toml` change, rebuild release, repeat the identical
    window, record the same two numbers.
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check`, `cargo test` — all still clean (a profile-only change; no
    source touched, so the 32+ test baseline from Phase 2 is expected to
    pass unedited).
  - **`git diff main --stat -- src/merge.rs` — zero diff, checkpoint 2 of
    3.** A `Cargo.toml`-only phase shouldn't touch it at all, but the check
    runs anyway per this packet's stated "at every phase that touches
    `src/aggregator.rs` or `src/model.rs`" rule's spirit — cheap to run,
    confirms no accidental source edit rode along with the profile change.
  - `git diff main --stat` overall — confirm only `Cargo.toml` touched
    since Phase 2.
- Done looks like: both release build times and both p50s recorded from
  real runs, ready to drop into Phase 6's README section — not a claim that
  the flags helped, whichever way the numbers actually land.
- Commit boundary: `Cargo.toml` alone. Reverting it (Phases 1-2 in place)
  returns to `strip = true` only — a real, working, slower-to-nothing
  release profile, with no effect on any other phase's work.

### Phase 4: Piece 2's decision — dedup implemented or not, per the measured rate — `src/aggregator.rs`

- Objective: act on the duplicate-rate number Phase 2 has been logging.
  If it's at or above ~30%, implement the skip; if below, leave the
  counter running and don't skip. Either outcome gets the real measured
  percentage recorded for Phase 6's README, not a rounded "high"/"low".
- Main changes:
  - `src/aggregator.rs`: read the duplicate percentage Phase 2's logging
    has already produced from a real run. If the skip is warranted: the
    `tx.send` call is made conditional on the freshly merged `Summary`
    differing from `last_published`'s contents; `last_published` itself
    still updates on every merge regardless of whether the send happens,
    per spec.md's explicit instruction — comparing against the true
    last-*merged* state, not the last-*sent* one, is the exact contract
    that keeps this different from (and correct where) a
    `lastUpdateId`-based scheme would be wrong. If the skip is not
    warranted, no `tx.send` call changes — the counter and comparison from
    Phase 2 stay exactly as they are, still logging, not acting.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — if the skip is implemented: a new unit or integration
    test in `src/aggregator.rs`'s `mod tests`, reusing the existing
    `watch::channel` + interleaved-send-and-read pattern from
    `a_venue_going_stale_narrows_the_published_summary` (send-then-await-
    changed, never batch sends, per that test's own load-bearing caution
    comment) — proving an unchanged tick does not produce a second
    `rx.changed()` wakeup. If the skip is not implemented: no new test is
    expected here; the existing Phase 2 tests already cover the comparison
    precondition, and there's no new behavior to assert on.
  - Existing baseline (Phase 2's histogram tests plus the 32-test floor)
    still passes unedited.
  - **`git diff main --stat -- src/merge.rs` — zero diff, checkpoint 3 of
    3 and the highest-risk checkpoint in this whole plan**, per spec.md's
    own framing: this is the phase where the dedup comparison, sitting
    directly next to `merge()`'s call site and comparing its own output,
    is most tempting to fold *into* `merge()` (e.g. "just have `merge()`
    take `last_published` and return `None` if unchanged"). Any nonzero
    diff here is a stop-and-flag condition — `merge()`'s purity contract
    (no clock, no I/O, no notion of "last published") is not up for
    revision in this packet.
  - `git diff main --stat` overall — confirm only `src/aggregator.rs`
    touched since Phase 3.
- Done looks like: the measured duplicate rate is acted on correctly per
  the ~30% threshold, whichever way it landed, with the decision and the
  real number both ready for Phase 6's README — and `src/merge.rs`
  confirmed unchanged across all three checkpoints in this plan.
- Commit boundary: `src/aggregator.rs` alone. Reverting it (Phases 1-3 in
  place) returns to Phase 2's measure-only state — the histograms and
  counter keep working, just nothing acts on the number, a safe fallback
  since spec.md's own Rollback Plan calls this a one-line revert either
  direction.

### Phase 5: load test at 100/500/1000 subscribers — `src/bin/loadtest.rs`, `Cargo.toml` (only if a new dependency is needed)

- Objective: a second `src/bin/` binary that opens N real gRPC connections
  against an already-running server and discards the stream while counting
  arrivals — replacing the README's current estimated saturation point
  with a measured curve.
- Main changes:
  - `src/bin/loadtest.rs`: a `clap`-parsed CLI (`--addr`, `--clients`,
    `--duration-secs`), following `src/bin/client.rs`'s existing pattern of
    reusing the generated
    `orderbook_aggregator_client::OrderbookAggregatorClient` rather than
    hand-rolling protocol handling. Spawns `--clients` independent tasks,
    each subscribing to `BookSummary` and counting messages received,
    without starting its own server (per spec.md's explicit Invariant —
    this measures a real, independently-running server, not an in-process
    shortcut). After `--duration-secs`, prints the aggregate receive rate
    and exits.
  - `Cargo.toml`: only touched if `loadtest.rs` needs a dependency not
    already present (e.g. nothing beyond `tokio`, `clap`, `tonic`, and the
    generated client is expected to be needed — confirm during
    implementation rather than pre-declaring one here).
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - No `cargo test` coverage expected for this binary — matching
    `008-client`'s standing "no tests for a manual-verification demo
    binary" convention, and spec.md's own Testing Strategy explicitly
    calling Piece 4 "not a `cargo test`."
  - Real runs, each against the real `aggregator` binary already running
    (via `docker compose up` or `cargo run --release`, matching how it's
    actually deployed): `cargo run --release --bin loadtest -- --addr
    http://127.0.0.1:50051 --clients 100 --duration-secs 60`, then 500,
    then 1000, sampling `docker stats <container>` (or the local process's
    CPU if not run under Docker — confirmed during implementation) during
    each run. Report the three (CPU%, sustained receive rate) pairs
    verbatim, not smoothed into a "linear, as expected" narrative if the
    shape doesn't actually look linear.
  - `git diff main --stat` overall — confirm only `src/bin/loadtest.rs`
    (and `Cargo.toml` only if genuinely needed) touched since Phase 4.
- Done looks like: three real (CPU%, publish-rate) measurements at
  100/500/1000 subscribers, ready to replace the README's estimate in
  Phase 6, with the load test never starting its own server.
- Commit boundary: `src/bin/loadtest.rs` (plus `Cargo.toml` if touched)
  alone. Reverting it has no effect on the `aggregator`/`client` binaries
  or on Phases 1-4's work — it's a new, optional, additive binary.

### Phase 6: README — prediction vs. result, all numbers, production notes — `README.md`

- Objective: close the loop the Prediction section opened — state the
  prediction, the measured result (including the specific parse-vs-rest
  percentage the prediction named, not just the total), the gap between
  them stated plainly, and every number from Phases 2-5.
- Main changes:
  - `README.md`, a new "Measurement" section (or extending the existing
    "Behaviour under load" subsection spec.md's Current State already
    describes): the prediction verbatim or paraphrased faithfully, the
    measured p50/p99/p99.9 for total/parse/merge+publish, the measured
    duplicate rate and Phase 4's decision, the release-profile before/after
    p50 and build times, the load-test table (100/500/1000 CPU%/rate), and
    an explicit statement of what ingest-to-publish latency is *not* (wire
    latency against the exchange — Binance's stream carries no event time,
    so cross-venue wire-latency comparison would be misleading).
  - The existing "roughly... low thousands of subscribers" estimate line
    and the "dedup deferred pending a measurement" line (both already in
    the README per Current State) are replaced with the real measured
    numbers and decision, not left standing alongside them.
  - Word budget: keep the README close to its current length — spec.md's
    Acceptance Criteria explicitly allows trimming elsewhere to make room,
    but no measured number or the prediction/result comparison gets cut to
    make the budget.
- Verification:
  - `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D
    warnings`, `cargo fmt --check` — full gate, all clean, at the tip of
    everything landed so far (documentation-only phase, but the standing
    convention in this project — `009-resilience` Phase 6, `007-merge`'s
    doc-pass commit — is to run the full gate at every README-touching
    phase, not skip it because "it's just docs").
  - `wc -w README.md` — report the actual word count and confirm it stays
    close to its pre-phase baseline.
  - Read the finished "Measurement" section back against Phases 2-5's
    actual recorded numbers line by line — confirm every number in the
    README matches what was actually observed running the phases, not a
    rounded or reconstructed approximation.
  - `git diff main --stat -- src/merge.rs` — zero diff, final confirmation
    at the tip (this phase touches no source, so this is a sanity check
    that nothing from Phases 2-5 slipped through unnoticed).
  - `git diff main --stat` overall — confirm the whole branch through this
    phase touches only `Cargo.toml`, `src/model.rs`,
    `src/exchange/binance.rs`, `src/exchange/bitstamp.rs`,
    `src/aggregator.rs`, `src/bin/loadtest.rs`, `README.md`, and
    `specs/011-measurement/` — no other path, and specifically not
    `src/merge.rs`, `src/server.rs`, or `proto/orderbook.proto`.
- Done looks like: a reader of the README alone can see the prediction, the
  real measured result, and the honest gap between them, without needing
  to read the spec packet to reconstruct what was measured.
- Commit boundary: `README.md` alone — matching this project's standing
  pattern (`009-resilience` Phase 6, `007-merge`) of landing a doc pass as
  its own, separately revertible commit.

### Phase 7: start the 24-hour run — no code change, a running process

- Objective: start Piece 5's 24-hour run against the fully merged build
  from Phases 1-6, not an intermediate one — explicitly sequenced last per
  spec.md's Order and Invariants sections.
- Main changes: none in `src/`. Operationally: `docker compose up` (or
  equivalent) started against a real pair (`ethbtc`) and left running,
  matching how the service is actually deployed.
- Verification:
  - Confirm the running build is built from the Phase 6 tip (`git rev-
    parse HEAD` recorded alongside the run's start time), not a
    Phase-2-or-earlier intermediate state.
  - Record the run's start time in `README.md` as its own small addition
    (a second, small commit on top of Phase 6's doc pass, not folded into
    it — the run may finish well after Phase 6's own review).
  - If the run completes before this packet's review/merge: write up
    reconnect counts per venue (including whether a reconnect lines up
    with Binance's ~24h forced-close boundary), p50/p99 at start vs. end,
    peak RSS, staleness-exclusion counts and durations, and the full-24h
    duplicate rate compared against Phase 4's short-workload rate — added
    to the same README section.
  - If the run has not completed by the time this packet is otherwise
    ready for review: the README states plainly that the write-up is
    pending, with the run's start time and expected completion — never
    presented as done before it is, per spec.md's explicit Invariant.
  - `git diff main --stat -- src/merge.rs` — zero diff (no code changed in
    this phase, so this is a final confirmation the whole branch never
    touched it).
- Done looks like: the 24-hour run is genuinely running against the shipped
  build, its start time is recorded, and the README honestly reflects
  whether the full write-up is in or still pending — not a description of
  a run that hasn't actually started, and not a result reported before the
  run that produced it actually finished.
- Commit boundary: a small `README.md` commit recording the start time (and
  later, if it lands before merge, a second small commit adding the
  completed write-up). Neither commit has any effect on runtime code —
  reverting either only removes documentation.

## Cross-Cutting Considerations

- **`src/merge.rs` zero-diff, checked at three points (Phase 2, Phase 4,
  Phase 6), not just claimed at the end.** This is the packet's single
  structural contract, inherited from `007-merge` (which first made
  `merge()` pure) and reaffirmed by every packet since
  (`009-resilience`'s six-checkpoint pattern for the same file). A
  regression introduced in Phase 4 — the phase under the most pressure to
  touch it, since the dedup comparison sits directly on top of `merge()`'s
  output — that only gets caught in Phase 6 is still a regression that
  shipped in Phase 4's own commit.
- **No new clock reaches `src/merge.rs`, ever, even indirectly through
  `Book`.** `Book` grows two new `Instant` fields in Phase 2, but
  `merge()`'s own code path never reads them — confirmed by the zero-diff
  check on `src/merge.rs` itself, not by inspecting `Book`'s field list.
- **Per-phase build/test/lint gate.** Every phase (except Phase 1, which
  makes no code change, and Phase 7, which is operational) runs `cargo
  build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`,
  and `cargo test` before being considered done — matching this project's
  standing per-phase discipline in `009-resilience` and `010-test-gaps`.
- **Piece 3 (release profile) is deliberately sequenced ahead of Piece 2's
  decision (Phase 4), per spec.md's explicit Order.** This is the one
  phase in this packet allowed to land before its own measurement is fully
  processed elsewhere — but it still gets measured before and after, not
  cargo-culted in on the strength of the flags' reputation alone.
- **Real workload, not a benchmark harness, is the verification instrument
  for Phases 3 and 5.** Per spec.md's Out of Scope, no benchmarking
  framework is added — the before/after and the load-test numbers both
  come from running the real binary against real or proxied exchange
  connections and a real gRPC client, accepting the resulting noise as
  real rather than smoothing it with a synthetic microbenchmark.
- **Piece 5 (Phase 7) does not block this packet's merge on 24 hours of
  wall-clock time.** Spec.md is explicit about this, and this plan treats
  it the same way — Phase 7's own "done" condition allows for a pending
  write-up, distinct from every other phase's "done" condition, which
  requires the real numbers to already be in hand.
- **Untouched-files discipline.** `src/server.rs`, `src/feed.rs`,
  `src/main.rs`, `proto/orderbook.proto`, `src/bin/client.rs`, `Dockerfile`,
  `compose.yml`, `rust-toolchain.toml` should all show zero diff at the tip
  of this branch — this packet's scope is the files named in spec.md's
  `related_paths` plus `src/exchange/binance.rs`/`bitstamp.rs` (needed for
  the two new timestamp stamps) and `src/bin/loadtest.rs` (new). A phase
  whose diff unexpectedly touches any of the untouched set is a stop-and-
  flag condition.

## Verification Gates

Before this branch is considered ready to hand off:

- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all clean at the tip of the branch (Phases 1-6; Phase
  7 is operational and doesn't gate this).
- `cargo test` reports the existing 32-test baseline plus every new test
  named in Phase 2 and (conditionally) Phase 4 above, each individually
  identifiable by its behaviour-sentence name — report the actual observed
  count, not an arithmetic guess.
- `hdrhistogram` confirmed as a `[dependencies]` entry, not
  `[dev-dependencies]`, at the tip.
- `git diff main --stat -- src/merge.rs` shows zero diff — checked
  identically at Phase 2, Phase 4, and Phase 6 (the three checkpoints
  above), not just once at the end.
- The prediction (spec.md, committed in Phase 1) and the measured result
  (README, Phase 6) both present, with the parse-vs-rest percentage the
  prediction specifically named — not just the total p50 — stated
  alongside the gap between prediction and result.
- Before/after p50 and before/after release build time both reported for
  the profile change (Phase 3), from real runs.
- The measured duplicate rate reported (Phase 2/4), and Phase 4's decision
  (skip implemented or not) matching that measured rate against the ~30%
  threshold.
- Load test results (CPU%, sustained publish rate) reported for 100, 500,
  and 1000 subscribers (Phase 5), from a load test that never starts its
  own server.
- The 24-hour run (Phase 7) started against the Phase 6 tip, with its
  start time recorded in the README — its full write-up present if it
  completed in time, explicitly marked pending if not.
- `wc -w README.md` reported and confirmed close to the pre-packet
  baseline.
- `git diff main --stat` at the tip shows only the files named in Cross-
  Cutting Considerations' "Untouched-files discipline" scope list — no
  other path, and specifically zero diff on `src/merge.rs`,
  `src/server.rs`, and `proto/orderbook.proto`.

## Expected Drift Triggers

If any of the following becomes true while implementing, update spec.md
before continuing rather than improvising past it:

- Recording the parse/merge+publish/total spans turns out to require
  passing a timestamp, a clock, or any `Instant`-carrying parameter into
  `src/merge.rs` (e.g. because `merge()`'s own return value needs to carry
  a "when computed" field) — this contradicts spec.md's central invariant
  and must be resolved by keeping the timing entirely in
  `src/aggregator.rs`, not by relaxing the invariant unilaterally.
- Adding `parse_started_at`/`parsed_at` to `Book` breaks `Book`'s derived
  `PartialEq`/`Eq` in a way the existing `src/model.rs` or `src/merge.rs`
  tests depend on (fixture equality assertions) — worth flagging rather
  than silently hand-rolling a `PartialEq` that ignores the new fields
  without recording that decision.
- The measured duplicate rate lands close to the ~30% threshold (e.g.
  25-35%) rather than clearly above or below it — spec.md gives a single
  approximate cutoff with no tie-breaking rule; worth a explicit call
  documented in the README rather than a silent rounding either direction.
- The load test (Phase 5) shows a clearly non-linear CPU/rate curve (e.g.
  a sharp knee well below 1000 subscribers) — spec.md already anticipates
  this as "the more interesting outcome" and asks for it to be reported
  plainly, not smoothed into a linear narrative; flag it prominently in
  the README rather than treating it as a run to discard and retry.
- The release-profile before/after comparison (Phase 3) shows a difference
  small enough to be plausibly explained by network jitter alone — spec.md
  already anticipates this risk; report the numbers honestly with that
  caveat stated, rather than treating a small delta as conclusive evidence
  either way.
- `docker compose up` or `docker stats` cannot be run at all in this
  environment (no Docker daemon, no route to either exchange even through
  the configured proxy) — report this as "not verified here" for whichever
  phase depends on it (Phase 5's CPU sampling, Phase 7's run), not silently
  omitted, matching every prior packet's standing rule in this project.
- The 24-hour run (Phase 7) reveals a reconnect, staleness exclusion, or
  RSS growth pattern that looks like a genuine bug rather than expected
  behaviour (e.g. unbounded RSS growth, a reconnect storm not explained by
  Binance's documented 24h boundary) — this is a real finding worth its
  own follow-up spec packet, not something to quietly absorb into this
  packet's README as if it were an expected number.
- This packet's own review/merge happens before Phase 7's run completes —
  expected and already accounted for in spec.md's Invariants; the trigger
  here is only if the pending-write-up note gets *skipped* rather than
  written, which would misrepresent an incomplete run as a finished one.
