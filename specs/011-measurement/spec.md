---
spec_name: "Step 9 — measurement"
spec_id: "011"
spec_folder: "011-measurement"
status: "approved"
created_at: "2026-08-26"
updated_at: "2026-08-26"
created_by: "claude"
creation_mode: "human-brief"
source_inputs:
  - "inputs/001-step-9-brief.md"
source_agents: []
goal: "Turn every latency/throughput claim currently made by argument (merged not sorted, borrowed parse, Arc in the watch, an arithmetic guess at subscriber-count ceiling) into a real measurement, written down as a prediction first, and act on two of those measurements (release profile, dedup) rather than assume the outcome."
purpose: "Steps 0-8 are correct and covered by 52 tests, but the brief lists speed as one of the most important operational factors and nothing about this codebase's speed has been measured yet. A design argument that has never been checked against a number is exactly the kind of claim this project's own conventions (measure the Bitstamp staleness gap, don't guess a grace period, confirm rather than assume) argue against making."
parent_request: "specs/011-measurement/inputs/001-step-9-brief.md (human brief, step 9 of the project's build order)"
related_paths:
  - "src/model.rs"
  - "src/aggregator.rs"
  - "src/merge.rs"
  - "src/server.rs"
  - "src/main.rs"
  - "src/bin/loadtest.rs"
  - "Cargo.toml"
  - "README.md"
verification_level: "mixed"
complexity: "medium"
---

# Spec: 011-measurement

## Problem

Every performance claim in the codebase and its README today is a design
argument, not a number: merging two sorted 20-element lists instead of
concatenating and sorting forty, borrowing `&str` out of the source JSON
instead of allocating a `String` per field, `Arc::clone` under the watch
lock instead of a deep clone, an *estimate* (explicitly labelled as such)
that the per-subscriber encode saturates a core somewhere in the low
thousands of subscribers. All plausible, none checked. Two decisions have
also been deliberately deferred pending a measurement that doesn't exist
yet: whether the top-ten dedup is worth building, and whether the release
profile's `lto`/`codegen-units` settings are worth their build-time cost.

## Goal

- A prediction for p50 ingest-to-publish latency and which pipeline stage
  dominates it, written down in this spec before any instrumentation
  exists (see Prediction, below) — not written after the number is known.
- Real ingest-to-publish latency instrumented via `hdrhistogram`, **split
  into a parse span and a merge+publish span** so the prediction's claim
  about which stage dominates is actually checkable, not just the total —
  reported as p50/p99/p99.9 for both spans plus the sustained update rate
  on a periodic log line, with `merge()` staying pure (no clock reaches
  it).
- The rate at which a freshly merged `Summary` is identical to the last
  one published, measured and acted on: dedup gets built only if the
  measured rate clears ~30%, and either way the number is reported.
- `[profile.release]` gains `lto = "fat"` and `codegen-units = 1`, with
  p50 measured before and after so the change has evidence behind it
  rather than being cargo-culted in.
- A load test at 100/500/1000 subscribers, recording CPU and sustained
  publish rate at each, replacing the README's estimated saturation point
  with a measured one (or showing the estimate was wrong, which is the
  more interesting outcome).
- A 24-hour run started against the fully merged build of everything
  above, recording reconnects (including whether Binance's 24h forced
  close shows up as one), p50/p99 drift from start to end, peak RSS,
  staleness exclusions, and the full-run dedup rate — written up in the
  README once it completes, without blocking this packet's merge on it.
- A README "Measurement" section stating the prediction, the result, the
  gap between them, all the numbers above, and explicitly what
  ingest-to-publish latency is *not* (wire latency — Binance's stream
  carries no event time, so only one venue could ever be compared against
  it, and comparing across venues on this metric would be misleading).

## Purpose

The brief names speed as operationally important. A reviewer reading this
codebase's design rationale (merge over concat-and-sort, borrowed parse,
`Arc` in the watch) is reading claims, not evidence — and this project's
own established pattern, used for the Bitstamp staleness threshold and the
grace period in `009-resilience`, is to measure before writing the number
down rather than asserting it. This step applies that same discipline to
the one area that has so far gotten away with argument alone: performance.

## Out of Scope

- **No custom `tonic` `Codec` implementation for pre-encoded bytes.** This
  is the real fix for the per-subscriber encode cost the load test will
  quantify. Deliberately left out: it means implementing an
  under-documented, version-unstable corner of `tonic`'s API for a gain
  the load test's own single-client run would show as zero. The load test
  produces the number that says how much it would be worth; the README
  states the number and that this was scoped and declined, not
  overlooked.
- **No `simd-json`**, unless Piece 1's measurement shows parsing at over
  60% of p50 — and even then, only *mentioned* as the next thing to try,
  not built, since a 2-3x speedup on a microsecond-scale absolute number
  is very unlikely to be operationally meaningful.
- **No buffer pooling.** Would save a couple of allocations per tick at
  the cost of `merge()`'s purity, which the existing 8 merge tests rely
  on (no clock, no I/O, no shared mutable state) — already weighed and
  rejected in an earlier step; this packet doesn't reopen it.
- **No metrics endpoint, no Prometheus exporter, no flamegraph tooling.**
  The deliverable is periodic log lines plus a README table, per the
  brief.
- **No `panic = "abort"`** in the release profile. It would remove the
  distinction `JoinError::is_panic()` currently gives `src/main.rs`'s
  `JoinSet` supervisor between "a task panicked" and "a task was
  cancelled" — those two cases are logged differently today, and losing
  that distinction is not something this step's profile change is
  entitled to cost.
- **No load-testing framework** (e.g. `goose`, `drill`). Piece 4's harness
  is a small dedicated binary that opens N real gRPC connections and
  discards the stream — nothing more general.
- **No change to `proto/orderbook.proto`.** Every measurement in this
  packet is server-side or process-level; nothing here touches the wire
  contract.

## Current State

Verified by reading the current source, not assumed:

- `src/model.rs`'s `Book` has no timestamp field today. `Price`/`Amount`
  parsing happens in each `Exchange::parse` (`src/exchange/binance.rs`,
  `src/exchange/bitstamp.rs`), both already borrowing `&str` fields out of
  the source JSON via `#[serde(borrow)]` rather than allocating per level
  — the ~27-allocation figure the brief cites for the parse stage is this
  existing borrowed-deserialization design, already landed, not something
  this step changes.
- `src/aggregator.rs`'s `run()` loop is exactly where a book arrives
  (`rx.recv()`), gets inserted into per-venue state, filtered by
  `fresh_venues`, and passed to `merge::merge()` before being published
  into the `watch::Sender<Option<Arc<Summary>>>`. This is the one place
  that already sees both "when a book arrived" and "when a `Summary` was
  published" — the natural, and only reasonable, home for both the
  latency recording and the dedup comparison, since `merge()` itself is
  required to stay pure (`.claude`'s rust rules and this project's own
  established pattern — no clock, no I/O reaches `src/merge.rs`, checked
  via `git diff main --stat -- src/merge.rs` at the end of every step
  that has touched this loop so far).
- `src/orderbook::Summary` and `src/orderbook::Level` (generated by
  `tonic-prost-build` from `proto/orderbook.proto`, confirmed by reading
  the generated `orderbook.rs` under `target/`) already derive
  `PartialEq` — comparing two `Summary` values with `==` is a real,
  already-available field-wise comparison (`spread: f64`, `bids: Vec<Level>`,
  `asks: Vec<Level>`, each `Level` also `PartialEq`), not something that
  needs a hand-written comparator. Since `spread`/`price`/`amount` are
  either passed through unchanged or rounded once to the fixed 8-decimal
  tick before publish, no two ticks that are genuinely identical books
  will differ by float noise — exact `==` is the right comparison, no
  epsilon needed here (unlike the *test* assertions elsewhere in this
  codebase, which do use an epsilon because they're comparing against a
  hand-computed expected value, not comparing two live-computed results
  against each other).
- `hdrhistogram = "7.6.0"` is currently a `[dev-dependencies]` entry in
  `Cargo.toml`, added in an earlier step specifically noting it would
  need to move once real production instrumentation needed it — which is
  what this step does.
- `Cargo.toml`'s `[profile.release]` today has only `strip = true`.
- `src/main.rs` spawns four tasks under one `JoinSet` today: two feeds,
  the aggregator, and the gRPC server (`server::router(rx).serve(addr)`).
  It is still the crate's primary binary — the original plan's rename to
  `src/bin/aggregator.rs` never happened. `src/bin/client.rs` does exist,
  landed in `008-client` as the demo client. There is no load-generation
  or benchmarking binary yet; Piece 4's `src/bin/loadtest.rs` will be the
  second entry under `src/bin/`, alongside `client.rs`.
- The README's "What I'd change for production" table and its "Behaviour
  under load" subsection (added in the immediately preceding
  documentation-only change, merged to `main`) already state, in an
  explicitly labelled *estimate*: "roughly — 20 publishes/s, microseconds
  each — saturates a core somewhere in the low thousands of subscribers,"
  and already document the dedup decision as deliberately deferred
  pending a measurement, with the exact two-line fix and the
  `lastUpdateId`-vs-published-`Summary` warning already written down.
  This step's job is to replace that estimate with Piece 4's measured
  curve and resolve that deferred dedup decision with Piece 2's measured
  rate — not to redesign either passage.
- Binance's `depth20@100ms` stream pushes roughly 10 messages/s per venue
  under normal conditions (already established by `src/feed.rs`'s
  `debug!` log line and by the Bitstamp staleness measurement taken in
  `009-resilience`); the brief's own "20 publishes/s" estimate in the
  README reflects both venues combined publishing independently through
  the same aggregator loop, not a single combined rate exchanges
  themselves produce.

## Prediction (written before any instrumentation exists)

Required by the brief: write this down first, before Piece 1 lands, so the
eventual measurement has something real to be compared against rather than
being "just a number."

**Prediction: p50 ingest-to-publish lands in the 5-25µs range, and the
dominant cost is JSON deserialization work in the parse stage — but not
because of its allocation count relative to the other two stages.**

Reasoning, from what's already known about each stage rather than a guess
pulled from nowhere:

- **Parse (~27 allocations/tick).** The allocation count alone
  understates this stage's cost. A `depth20` message is roughly 1.5-2KB of
  JSON text carrying 40 levels (`20 bids + 20 asks`) × 2 numeric-string
  fields each. Even with the borrow already eliminating per-field
  `String` allocation, `serde_json` still has to walk and validate that
  entire byte range and parse ~80 floating-point literals out of it per
  message per venue — work whose cost scales with message size and
  content, not primarily with allocation count. That work is expected to
  be the largest single contributor to p50, which is why the prediction
  names parse as the dominant stage even though the raw allocation counts
  (27 vs. 22) don't look far apart.
- **Build the `Summary` (~22 allocations/tick).** Roughly comparable
  allocation count to parse, but each allocation here is a cheap `Vec`
  push or a short `String::from` (the `Level.exchange` field, via
  `Venue::to_string()` in `src/merge.rs`) — no parsing, no validation, no
  variable-length text to walk. Expected to cost noticeably less
  wall-clock time than parse despite the similar allocation count,
  because the *kind* of work per allocation is cheaper.
- **Merge (~20 comparisons).** Expected to be the smallest of the three
  by a wide margin. A `Price`/`Amount` comparison is `f64::total_cmp` — a
  handful of CPU instructions, no allocation, no branching on variable-
  length input. Twenty of those is expected to cost low hundreds of
  nanoseconds at most, likely under 5% of total p50.

If the measurement instead shows merge or the Summary-building stage
dominating, or shows a p50 well outside 5-25µs in either direction, that
contradicts this reasoning and is the more interesting result — worth
writing up honestly rather than rationalized after the fact to fit.

**This prediction names which stage dominates, so what gets measured has
to be split the same way — a single total-latency number can't confirm or
falsify it.** Piece 1, below, records two spans, not one: parse
(`parsed_at - parse_started_at`) and everything after
(`published_at - parsed_at`). The README line this makes possible is "I
expected parsing to dominate. It did, at X% of p50" (or "it didn't") —
not just a total with a story attached to it after the fact.

## Proposed Design

### Piece 1 — ingest-to-publish latency, split into parse and merge+publish

Two timestamps thread through `Book`, not one, so the prediction's claim
about which stage dominates is checkable rather than just the total:

- Add `parse_started_at: Instant` and `parsed_at: Instant` to `Book`
  (`src/model.rs`). Each `Exchange::parse` implementation
  (`src/exchange/binance.rs`, `src/exchange/bitstamp.rs`) stamps
  `parse_started_at` with `Instant::now()` at the very top of `parse` —
  before `serde_json::from_str` runs — and stamps `parsed_at` once
  parsing has actually succeeded, immediately before returning
  `Some(Book { .. })`.
- `src/aggregator.rs`'s `run()` loop, on each `Some((venue, book))`
  received, records into **two** separate `hdrhistogram::Histogram`s:
  - **parse span**: `book.parsed_at.duration_since(book.parse_started_at)`
    — recorded as soon as the book is received, independent of whether it
    ends up published.
  - **merge+publish span**: `Instant::now().duration_since(book.parsed_at)`
    — recorded **after** `merge::merge()` returns `Some` and the
    `watch::Sender::send` call actually happens, matching Piece 1's
    original "publish means it reached the channel" rule: a book that
    gets filtered out by staleness, or (once Piece 2 lands) merges into
    an unchanged `Summary` that gets skipped, was never actually
    published and contributes no sample to this span.
  - The **total** ingest-to-publish figure the README headlines is
    `published_at - parse_started_at`, read directly off a third
    timestamp taken at the same `send` call, not derived by adding the
    two histograms' percentiles together — percentiles aren't additive
    across independent distributions, so a real third measurement is the
    only correct way to report a total p50/p99/p99.9.
  - **What the merge+publish span actually covers, stated plainly in the
    README next to the number**: not just `merge()`'s own comparisons —
    it also includes the time the book spent sitting in the bounded
    `mpsc` channel behind any other message ahead of it, and the
    `fresh_venues` staleness filter. If this span comes out surprisingly
    large, that's queueing or filtering, not `merge()` being slow — check
    the update rate and channel depth before concluding anything about
    merge's own cost, and say so in the README rather than mislabeling
    the span as "merge time."
- The three histograms, plus a running count of updates and (once Piece 2
  lands) a running count of `send`s skipped as duplicates, live as
  `Aggregator`-owned state (same single-owner, no-`Arc<Mutex<_>>` pattern
  already used for `venues`) and get logged on a `tokio::time::interval`
  tick roughly every 30s: p50/p99/p99.9 for total, parse, and
  merge+publish (all read from the histograms, not recomputed by hand),
  the sustained update rate (`updates this window / window duration`),
  and the dedup percentage (Piece 2).
- **`src/merge.rs` does not change.** No `Instant`, no clock, no new
  parameter reaches `merge()` or `merge_side()` — every timestamp is read
  and recorded entirely in `src/exchange/*.rs` (the two parse stamps) and
  `src/aggregator.rs` (the recording and the publish stamp), around the
  existing `merge::merge(&fresh)` call, exactly as `009-resilience`'s
  staleness filter already keeps the clock in `fresh_venues` and out of
  `merge()`. This is the single most load-bearing invariant in this
  packet — see Invariants below.
- `hdrhistogram` moves from `[dev-dependencies]` to `[dependencies]` in
  `Cargo.toml`.

### Piece 2 — dedup rate, measured, then acted on

- Alongside the histogram, `Aggregator` keeps `last_published:
  Option<Arc<Summary>>` (already has the `Arc<Summary>` shape available
  from what it sends into the `watch` channel).
- On every tick where `merge::merge()` returns `Some(summary)`: compare
  `summary` (by value, via `Summary`'s derived `PartialEq` — see Current
  State) against `last_published`'s contents. If equal, increment a
  "would-have-been-a-duplicate" counter and (see below) either still send
  it or skip it, depending on which side of the decision this lands on.
  If different, send it and update `last_published`.
- **The measurement runs before the decision is made, not after.** The
  counter above is wired in as soon as Piece 1's instrumentation lands,
  and logged in the same periodic line, *before* any send is actually
  skipped. Once a real measured percentage exists:
  - If the measured duplicate rate is at or above ~30%, implement the
    skip: the `watch::send` call for that tick is not made, and
    `last_published` still updates so the *next* comparison is still
    against the true last-merged state, not the last-*sent* one — the
    two must be kept identical here, since skipping a send while still
    updating `last_published` from the un-sent value is exactly the
    "compare against the published `Summary`" contract the README
    already promises, not `lastUpdateId`-style per-venue bookkeeping.
  - If it's below ~30%, no send is skipped — `last_published` still
    tracks the comparison for the log line's percentage, but the
    dedup counter's existence keeps measuring, not acting.
  - Either outcome is recorded in the README with the actual measured
    percentage, not a rounded-off "high" or "low."
- **`src/merge.rs` still does not change.** The comparison and the
  decision both live in `src/aggregator.rs`, after `merge()` returns —
  `merge()` has no notion of "last published," matching its existing
  purity contract and this project's stated split (dedup is the
  aggregator's job, deliberately not folded into `merge()`, since step 5
  landed real merge logic).

### Piece 3 — release profile

- Add to `Cargo.toml`:

  ```toml
  [profile.release]
  strip = true
  lto = "fat"
  codegen-units = 1
  ```

- Measured, not assumed to help: build the release binary before this
  change and after, run the same repeatable local workload (real
  Binance + Bitstamp connections for a fixed window, e.g. 5 minutes, with
  Piece 1's histogram already in place) against each build, and report
  both p50s side by side in the README, plus the release build time
  before/after (an `lto = "fat"` + `codegen-units = 1` build is markedly
  slower to compile — worth stating alongside the runtime number, since
  it's a real cost of the change, not a hidden one).
- `panic = "abort"` is explicitly not added — see Out of Scope.

### Piece 4 — load test at 100 / 500 / 1000 subscribers

- New binary, `src/bin/loadtest.rs` — the crate's second `src/bin/`
  binary (the first being the existing `src/bin/client.rs` demo client
  from `008-client`). A `clap`-parsed CLI (`--addr`, `--clients`,
  `--duration-secs`), reusing the existing generated
  `orderbook_aggregator_client::OrderbookAggregatorClient` the way
  `src/bin/client.rs` already does.
- Behavior: connect `--clients` independent gRPC connections to a
  already-running `--addr` server (the load test does not start its own
  server — it's meant to be pointed at the real `aggregator` binary,
  already streaming real exchange data, so the server side of the
  measurement reflects genuine production load rather than a synthetic
  server-and-client-in-one-process shortcut), each subscribing to
  `BookSummary` and discarding every message it receives while counting
  how many arrived. After `--duration-secs`, print the aggregate receive
  rate across all clients and exit.
- CPU is read from outside the Rust process, not instrumented in code:
  this project already runs the server under Docker (`compose.yml`), so
  `docker stats <container>` sampled during each of the three runs is the
  simplest accurate source for the server's CPU% under load — no new
  dependency, no self-profiling code, consistent with "keep the harness
  simple" and "no metrics endpoint" from Out of Scope. The three sampled
  CPU%/publish-rate pairs (100/500/1000 clients) go into a README table.
- If the curve looks linear, state that the estimate in the current
  README is now a measurement. If it doesn't, report the actual shape and
  flag it as worth a closer look rather than silently smoothing it into
  the "linear, as expected" narrative.

### Piece 5 — the 24-hour run

- Started only after Pieces 1-4 are merged, so it exercises the shipped
  build, not an intermediate one — this is explicitly sequenced last in
  Order, below.
- Run the real `aggregator` binary (via `docker compose up`, matching how
  it's actually deployed) for 24 hours against a real pair (`ethbtc`),
  with Piece 1's periodic log line as the primary data source plus a
  process-level RSS sample taken periodically (`docker stats` again, or
  `/proc/<pid>/status`'s `VmRSS` inside the container — whichever is
  simpler to script; decided during implementation, not guessed here).
- Recorded, from the logs and samples: reconnect count per venue (grepping
  `src/feed.rs`'s existing `"connected"`/`"reconnecting after backoff"`
  log lines), whether one of Binance's reconnects lines up with the
  documented ~24h forced-close boundary, p50/p99 at the start of the run
  vs. the end (drift would indicate a leak or something accumulating),
  peak RSS, how many times a venue was excluded as stale and for how long
  (from `src/aggregator.rs`'s existing staleness path — no new log line
  needed if the existing ones are sufficient; add one if they aren't),
  and the full-24h dedup rate compared against the short local-workload
  rate from Piece 2/3's testing.
- The run is *started* as part of this packet's work and its result is
  written into the README once it completes — but this packet's own
  merge is not blocked on 24 hours of wall-clock time. The README section
  documents the run's start time and, if this packet's own review/merge
  happens before the run finishes, states plainly that the write-up is
  pending and will land as a follow-up update once it completes — not
  reported as done before it is.

## Order

1. This spec (prediction included) — first commit, alone.
2. Piece 1: latency instrumentation + dedup-rate counting (measurement
   only, no skip yet).
3. Piece 3: release profile change, p50 measured before and after (kept
   ahead of Piece 2's action-on-measurement per the brief's explicit
   sequencing — the profile change is the one thing in this packet
   allowed to be free/low-risk enough to land before its own measurement
   is fully processed, and even it still gets measured before and after).
4. Piece 2's decision: dedup implemented, or not, per the measured rate.
5. Piece 4: load test at 100/500/1000.
6. README: prediction vs. result, all measured numbers, updated
   production notes.
7. Piece 5: start the 24-hour run; finish/ship the README write-up for it
   once it completes.

## Acceptance Criteria

- A periodic (~30s) log line reporting p50, p99, p99.9 for total
  ingest-to-publish latency **and** the parse and merge+publish spans
  separately, the sustained update rate, and the dedup percentage.
- `hdrhistogram` is a `[dependencies]` entry, not `[dev-dependencies]`.
- `git diff main --stat -- src/merge.rs` shows no diff at every phase of
  this packet.
- Before-and-after p50 reported for the release-profile change, plus the
  before/after release build time.
- Load test results (CPU%, sustained publish rate) reported for 100, 500,
  and 1000 subscribers.
- The prediction (this spec) and the measured result (README) are both
  present, with the gap between them stated plainly — not silently
  reconciled — including the specific parse-vs-rest percentage the
  prediction named, not just the total.
- The 24-hour run is started, with its start time recorded in the README,
  even if its full write-up lands as a follow-up.
- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D
  warnings`, `cargo fmt --check` all pass.
- README stays close to its current length: this section adds real
  content, so some trimming elsewhere is expected, but no measured number
  or the prediction/result comparison is cut to make the budget.

## Invariants and Critical Don'ts

- **`src/merge.rs` takes no clock, ever.** Not `Instant::now()`, not a
  parameter carrying a timestamp, not a `SystemTime`. Both the latency
  recording (Piece 1) and the dedup comparison (Piece 2) live entirely in
  `src/aggregator.rs`, wrapped around the existing `merge::merge()` call
  — this is the exact rule this step is most likely to break "out of
  convenience," named as such in the brief, and it is the first thing
  checked before any commit in this packet is considered done.
- Dedup's `last_published` comparison is against the merged `Summary`,
  never against a per-venue `lastUpdateId` — a venue's 15th level can
  change without moving the merged top ten, so comparing sequence numbers
  would produce a different (wrong) duplicate rate than comparing the
  actual output.
- No `panic = "abort"` in the release profile — see Out of Scope for why.
- The load test (Piece 4) is a thin client that measures a real,
  independently-running server; it must not spin up its own in-process
  server, which would measure something other than production load.
- The 24-hour run's write-up is reported as pending, not as complete,
  if this packet's review happens before the run finishes. No number from
  an incomplete run gets presented as the final figure.

## Risks and Tradeoffs

- The release-profile measurement (Piece 3) uses a live network workload
  (real exchange connections over a fixed window), which is inherently
  noisier than a synthetic benchmark — network jitter, not just the
  compiler flag, will show up in the before/after comparison. Mitigated
  by running both builds against the same kind of window and reporting
  the numbers honestly rather than treating a small difference as
  conclusive; a synthetic in-process benchmark was considered and
  rejected as adding a dependency/harness this step's scope (Out of
  Scope: no benchmarking framework) doesn't call for.
- CPU sampling via `docker stats` (Piece 4) is coarser than an in-process
  CPU-time measurement would be — acceptable per the brief's own "doesn't
  need to be precise, needs to be honest about the shape of the curve."
- The load test's 1000-subscriber run adds real load to whatever machine
  runs it; if that's the same host running other work, the CPU% reading
  could be confounded by unrelated load. Documented as a caveat in the
  README next to the number, not silently assumed away.
- Piece 5's 24-hour run is real wall-clock time this packet's merge does
  not wait on — there's a real chance the write-up lands after this
  packet is otherwise done and reviewed. Stated plainly in Acceptance
  Criteria and Invariants rather than treated as a soft deadline to rush.

## Testing Strategy

Required real verification:

- Piece 1: a unit test in `src/aggregator.rs` asserting that a `Book` with
  known `parse_started_at`/`parsed_at` values produces a recorded sample
  in **both** the parse histogram and the merge+publish histogram once
  merged and published, and that a book which never gets published (e.g.
  filtered out as stale) contributes to neither the merge+publish
  histogram nor the total — using hand-computed `Instant`s, matching the
  file's existing "pass the clock in, don't call `Instant::now()`
  internally where testability requires it" pattern already used for
  `fresh_venues`/`past_grace`.
- Piece 2: a unit test asserting two structurally identical merged
  `Summary`s (same venues, same books) compare equal via `Summary`'s
  `PartialEq`, and a second asserting a changed book (one different
  price) compares unequal — both are the actual precondition the dedup
  logic depends on, not a restatement of `prost`'s own derive (locking in
  that this project's specific usage produces the comparison behavior
  the dedup code assumes, the same "confirm, don't assume" reasoning
  `009-resilience` applied to `subscribe_message`'s reconnect timing).
- Piece 2 (if the measured rate crosses the threshold and the skip is
  implemented): a unit or integration test proving an unchanged tick is
  *not* re-sent down the `watch` channel — reusing the existing
  `watch::channel` + interleaved-send-and-read pattern already
  established in `src/aggregator.rs`'s
  `a_venue_going_stale_narrows_the_published_summary` test, to avoid the
  same batched-send deadlock documented there.
- Piece 3: not unit-testable (a compiler/linker flag's effect can't be
  asserted in `cargo test`) — verified by the before/after measurement
  itself, reported in the README with real numbers from real runs.
- Piece 4: not a `cargo test` — a manually-run binary against a manually
  observed metric (`docker stats`), consistent with the brief's explicit
  "not a benchmarking framework" instruction. The three runs and their
  results are reported in the README as the verification.
- Piece 5: the 24-hour run itself is the verification for step 7's
  reconnection design — no unit test substitutes for actually holding a
  connection open against Binance's real 24h boundary.

Optional supporting checks:

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` (already
  listed under Acceptance Criteria).

## Rollback Plan

Pieces 1-3 are additive and independently revertible: the
`parse_started_at`/`parsed_at` fields, the histograms, the dedup
counter/skip, and the release-profile
change each touch a small, separable surface (`src/model.rs`,
`src/aggregator.rs`, `Cargo.toml`) and none changes the wire contract or
`src/merge.rs`. Piece 4 is a new, optional binary with no effect on the
existing `aggregator`/`client` binaries if unused. Piece 5 is a running
process, not a code change — stopping it has no rollback implications.
If the dedup skip (Piece 2) turns out to be wrong after landing, reverting
it is a one-line change (send unconditionally again) with no migration.

## Open Questions

None blocking the start of implementation. Two things are explicitly
decided-during-implementation rather than guessed here, both already
flagged in Design above as such: whether the 24-hour run's RSS sampling
uses `docker stats` or `/proc/<pid>/status` (a mechanical choice with no
behavioral consequence either way), and the exact wording/placement of the
24-hour run's "pending" note in the README if this packet's review lands
before that run completes.
