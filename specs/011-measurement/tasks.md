# Tasks: 011-measurement

## Task Writing Rules

- Each task should describe a real unit of progress.
- Each task should name the expected files or areas touched.
- Each task should include explicit verification.
- Prefer behavior-level verification over mock-only checks.

Seven phases below map 1:1 to plan.md's own phase breakdown, which itself
copies spec.md's fixed "Order" section verbatim — do not reorder phases, and
do not fold a later phase's work into an earlier commit even if a file is
already open.

**The one check that repeats across this entire packet:**
`git diff main --stat -- src/merge.rs` must show zero diff. This project's
merge logic is required to stay pure — no clock, no I/O, no notion of "last
published" reaches it — and this packet is explicitly the one most likely to
break that rule "out of convenience," since every span recorded and every
dedup comparison wraps directly around the existing `merge::merge()` call
site. The check is run at the end of every task that touches
`src/aggregator.rs` or `src/model.rs`, not just once at the end.

None of this packet's explicitly out-of-scope mechanisms — a custom `tonic`
`Codec` for pre-encoded bytes, `simd-json` (unless Task 3's measurement shows
parsing over 60% of p50), buffer pooling, a metrics/Prometheus endpoint, a
flamegraph tool, `panic = "abort"`, or a general load-testing framework — may
appear in any task below. If implementation drifts toward one mid-task, stop
and flag it rather than treating it as a natural extension.

## Phase 1: the prediction is already committed — confirm, don't redo

### 1. Confirm the prediction predates any instrumentation

- Files or areas: none (verification-only task, no source or spec edit).
- Change: none.
- Verification:
  - `git log --oneline specs/011-measurement/spec.md` — confirm the spec
    (with its "Prediction" section, stating p50 in the 5-25µs range with
    parse as the dominant stage) has its own commit, and that no later
    commit on this branch edits the numbers in that section.
  - Quote the commit hash and message found.
- Done when:
  - The prediction's commit is confirmed to exist and to predate Phase 2's
    first commit — real evidence the prediction was written down first, not
    a claim taken on faith.
- Commit boundary: none — produces no commit.

## Phase 2: latency instrumentation + dedup-rate counting — `src/model.rs`, `src/exchange/binance.rs`, `src/exchange/bitstamp.rs`, `src/aggregator.rs`, `Cargo.toml`

### 2. Move `hdrhistogram` to `[dependencies]`

- Files or areas: `Cargo.toml` only.
- Change: move the existing `hdrhistogram = "7.6.0"` entry from
  `[dev-dependencies]` to `[dependencies]`, updating the comment above it
  (currently explains it's dev-only "for now") to state it now backs
  production latency instrumentation in `src/aggregator.rs`, per this
  packet.
- Verification:
  - `cargo build` — clean, confirms the crate still resolves with the entry
    moved.
  - Read `Cargo.toml` back and confirm `hdrhistogram` appears once, under
    `[dependencies]`, not under `[dev-dependencies]`.
- Done when:
  - `hdrhistogram` is a `[dependencies]` entry and the stale "dev-only for
    now" comment is corrected.

### 3. Add `parse_started_at`/`parsed_at` to `Book` and stamp them in both parsers

- Files or areas: `src/model.rs`, `src/exchange/binance.rs`,
  `src/exchange/bitstamp.rs`.
- Change:
  - `src/model.rs`: add `pub parse_started_at: std::time::Instant` and
    `pub parsed_at: std::time::Instant` fields to `Book`. `Instant` derives
    `PartialEq`/`Eq`/`PartialOrd`/`Ord` in `std`, so `Book`'s existing
    `#[derive(Clone, Debug, PartialEq, Eq)]` continues to compile without
    edits — confirm this by building, don't assume. Before touching
    anything else, grep `src/merge.rs` and `src/aggregator.rs`'s test
    modules for any `assert_eq!` comparing two full `Book` values (not just
    their `bids`/`asks` fields) — none currently exist (confirmed during
    planning), so no fixture is expected to need a hand-written `PartialEq`
    that ignores the two new timing fields; if one is found during this
    task, resolve by hand-implementing `PartialEq` on `Book` to compare only
    `bids`/`asks`/`last_update_id`, and note that decision in a comment on
    the impl rather than silently dropping the derive.
  - `src/exchange/binance.rs`: in `Binance::parse`, stamp
    `parse_started_at: Instant::now()` as the very first statement in the
    function body, before `serde_json::from_str` runs. Stamp
    `parsed_at: Instant::now()` immediately before the final `Some(Book {
    .. })` return — not on any early `None` return (a non-book message
    records neither timestamp, matching the function's existing "not every
    message is a book" contract).
  - `src/exchange/bitstamp.rs`: identical treatment in `Bitstamp::parse` —
    same two stamp points, same "only on the success path" rule.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build` — clean.
  - `cargo test` — the existing fixture-based tests in both
    `src/exchange/binance.rs` and `src/exchange/bitstamp.rs` (e.g.
    `parses_depth20_into_twenty_bids_and_twenty_asks_with_correct_values`,
    the Bitstamp equivalent) still pass unedited — confirms adding the two
    fields didn't require touching any existing assertion.
  - `git diff main --stat -- src/merge.rs` — zero diff.
- Done when:
  - `Book` carries both timestamps, both parsers stamp them only on the
    success path, and every pre-existing parser test still passes without
    modification.

### 4. Add three histograms, the update/duplicate counters, and periodic logging to `Aggregator`

- Files or areas: `src/aggregator.rs` only.
- Change:
  - `Aggregator` gains: `parse_histogram: hdrhistogram::Histogram<u64>`,
    `merge_publish_histogram: hdrhistogram::Histogram<u64>`,
    `total_histogram: hdrhistogram::Histogram<u64>` (recording nanoseconds,
    matching `hdrhistogram`'s own convention of an integer-count metric —
    confirm the exact constructor/`record` signature against the resolved
    0.14 API on docs.rs rather than assuming an older signature),
    `update_count: u64`, `duplicate_count: u64`, and
    `last_published: Option<Arc<Summary>>`.
  - In `run()`'s `Some((venue, book))` arm, before the existing
    `aggregator.venues.insert(...)` call: record
    `book.parsed_at.duration_since(book.parse_started_at)` into
    `parse_histogram` unconditionally — every received book contributes a
    parse-span sample regardless of whether it ends up published.
  - After the existing `merge::merge(&fresh)` call returns `Some(summary)`
    and immediately before the `tx.send(...)` call: compare `summary`
    against `aggregator.last_published`'s inner value via `Summary`'s
    derived `PartialEq`. If equal, increment `duplicate_count` — no send is
    skipped yet, this phase measures only. Update `last_published` to
    `Some(Arc::new(summary.clone()))` (or reuse the same `Arc` about to be
    sent) on every merge, whether or not it was a duplicate, per the
    contract Phase 4 (dedup decision) depends on. After the `tx.send` call
    actually happens: record
    `Instant::now().duration_since(book.parsed_at)` into
    `merge_publish_histogram`, record
    `Instant::now().duration_since(book.parse_started_at)` into
    `total_histogram` (a fresh `Instant::now()` call for the total span, not
    derived by summing the other two histograms' recorded values —
    percentiles aren't additive), and increment `update_count`.
  - A book filtered out by `fresh_venues` (i.e. `merge::merge` returns
    `None`, or the venue that produced this book was excluded before
    reaching `merge`) contributes its parse-span sample (recorded
    unconditionally above) but no sample to `merge_publish_histogram` or
    `total_histogram`, and no duplicate-count increment.
  - Add a `tokio::time::interval` (30s) inside `run()`'s existing
    `tokio::select!`, alongside the current `rx.recv()` and `grace_check`
    arms, that on tick logs one `tracing::info!` line with: p50/p99/p99.9 in
    microseconds for `total_histogram`, `parse_histogram`, and
    `merge_publish_histogram` (via `Histogram::value_at_quantile`), the
    sustained update rate (`update_count` delta since the last tick divided
    by the tick interval), and the running duplicate percentage
    (`duplicate_count as f64 / update_count as f64 * 100.0`, guarded against
    `update_count == 0`).
  - `src/merge.rs` receives no new parameter, no `Instant`, no clock of any
    kind — every read/write above lives in `src/aggregator.rs` only,
    wrapped around the existing `merge::merge(&fresh)` call.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build` — clean.
  - `cargo test` — four new tests in `src/aggregator.rs`'s `mod tests`,
    following the file's existing "pass timestamps in by hand, don't call
    `Instant::now()` inside the function under test" pattern (`past_grace`,
    `fresh_venues`):
    - `a_published_book_records_both_parse_and_merge_publish_samples` —
      construct a `Book` with hand-computed `parse_started_at`/`parsed_at`
      a known `Duration` apart, drive it through the same
      `fresh_venues`/`merge::merge`/record sequence `run()`'s loop body
      performs, and assert both `parse_histogram` and
      `merge_publish_histogram` (or the `Aggregator`'s exposed counts, if
      the histograms themselves aren't easily inspected — expose whatever
      minimal accessor the test genuinely needs) report exactly one sample
      each.
    - `a_stale_book_records_no_merge_publish_or_total_sample` — reusing the
      existing `stale_venue_excluded_from_merge` fixture shape, assert a
      book filtered out by `fresh_venues` contributes to neither
      `merge_publish_histogram` nor `total_histogram` (parse-span sample is
      still expected, per the unconditional recording rule above).
    - `two_structurally_identical_summaries_compare_equal` — build two
      merged `Summary` values from the same venues and the same book
      contents and assert `==` holds; the actual precondition the duplicate
      counter's comparison depends on, not a restatement of `prost`'s own
      derive.
    - `a_changed_book_produces_a_summary_that_compares_unequal` — same setup
      with one book's price changed, assert the two merged `Summary`s
      compare unequal.
  - Existing 32-test baseline (25 unit, 6 + 1 integration) still passes
    unedited.
  - Manual: `cargo run -- --pair ethbtc --port 50051` with `RUST_LOG=info`
    against real (or proxied) connectivity for at least 90s; confirm the
    periodic log line appears roughly every 30s with non-degenerate
    p50/p99/p99.9 (not all zero, not all identical across the three
    histograms) and a duplicate percentage that changes between log lines.
    Quote the actual observed log lines.
  - **`git diff main --stat -- src/merge.rs` — zero diff.**
  - `git diff main --stat` overall — confirm only `src/model.rs`,
    `src/exchange/binance.rs`, `src/exchange/bitstamp.rs`,
    `src/aggregator.rs`, `Cargo.toml` (plus `specs/011-measurement/`)
    touched since `main`.
- Done when:
  - A real periodic log line reports three genuinely distinct
    non-degenerate histograms fed from live data, the duplicate counter
    measures without skipping any send, all four new tests pass by name
    alongside the unedited 32-test baseline, and `src/merge.rs` shows zero
    diff.
- Commit boundary: tasks 2-4 land together as one commit (the histogram
  fields, the timestamp stamping, and the counter all depend on each other
  to be meaningfully testable in isolation). Reverting it returns to the
  pre-instrumentation aggregator — buildable, functioning, but with no
  latency or dedup visibility.

## Phase 3: release profile, measured before and after — `Cargo.toml`

### 5. Add `lto = "fat"` and `codegen-units = 1`, measure before and after

- Files or areas: `Cargo.toml` only.
- Change: add `lto = "fat"` and `codegen-units = 1` to the existing
  `[profile.release]` block, alongside `strip = true`. Do **not** add
  `panic = "abort"` — spec.md's Out of Scope excludes it explicitly, since
  it would collapse `JoinError::is_panic()`'s panic-vs-cancellation
  distinction in `src/main.rs`'s `JoinSet` supervisor.
- Verification:
  - **Before**: at the Phase 2 tip (before this task's `Cargo.toml` edit),
    time `cargo build --release` (e.g. `time cargo build --release`) and
    record the wall time. Run the release binary
    (`./target/release/keyrock-case-study --pair ethbtc --port 50051`)
    against real or proxied connectivity for a fixed 5-minute window with
    `RUST_LOG=info`, and record the total-span p50 from the last periodic
    log line printed inside that window.
  - **After**: apply the `Cargo.toml` change, repeat both measurements
    (build time, 5-minute-window p50) identically.
  - Report all four numbers (build time before/after, p50 before/after)
    verbatim, whichever direction they land — this task is not permitted to
    round a small delta into "the flags helped" or "the flags didn't help"
    without stating the actual numbers.
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check`, `cargo test` — all clean; no source file changed, so the
    Phase 2 test count is expected to pass unedited.
  - `git diff main --stat -- src/merge.rs` — zero diff (a `Cargo.toml`-only
    task shouldn't touch it; cheap to reconfirm).
  - `git diff main --stat` overall — confirm only `Cargo.toml` changed
    since Phase 2.
- Done when:
  - Both release build times and both 5-minute-window p50s are recorded
    from real runs, ready for Phase 6's README, and `src/merge.rs` shows
    zero diff.
- Commit boundary: `Cargo.toml` alone. Reverting it returns to
  `strip = true` only — a working, slower-to-build-nothing release profile,
  with no effect on any other phase's work.

## Phase 4: dedup decision — implement the skip, or don't, per the measured rate — `src/aggregator.rs`

### 6. Act on the measured duplicate rate against the ~30% threshold

- Files or areas: `src/aggregator.rs` only.
- Change:
  - Read the duplicate percentage Phase 2's periodic log line has already
    produced from a real run (reuse Task 5's or a fresh 5-minute run's
    output — record the actual observed percentage in the commit/report,
    not a rounded "high"/"low").
  - **If the measured rate is at or above ~30%:** make the `tx.send(...)`
    call conditional on the freshly merged `Summary` differing from
    `last_published`'s contents (the comparison Task 4 already wired in).
    `last_published` continues to update on every merge regardless of
    whether the send happens — comparing the *next* tick against the true
    last-*merged* state, not the last-*sent* one, is the exact contract
    that distinguishes this from a `lastUpdateId`-based scheme.
  - **If the measured rate is below ~30%:** no change to the `tx.send` call
    — the counter and comparison already wired in Task 4 keep running,
    still logging, not acting. State explicitly (in the commit message and
    the report) which branch was taken and the number that decided it.
  - **If the measured rate lands close to the threshold (roughly 25-35%),**
    per plan.md's own drift trigger: make an explicit call, document it in
    the README (Phase 6) with the actual number and the reasoning, rather
    than silently rounding either direction.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` — clean.
  - `cargo test`:
    - If the skip was implemented: one new test,
      `an_unchanged_tick_is_not_resent_down_the_watch_channel`, reusing the
      existing `watch::channel` + interleaved-send-and-read pattern from
      `a_venue_going_stale_narrows_the_published_summary` (send, then
      `.changed().await`, before the next send — never batch sends, per
      that test's own load-bearing caution comment) — proving a second,
      identical merge does not produce a second `rx.changed()` wakeup.
    - If the skip was not implemented: no new test expected; Task 4's tests
      already cover the comparison precondition and there's no new send
      -skipping behavior to assert on.
  - Existing baseline (Phase 2's four new tests plus the 32-test floor)
    still passes unedited.
  - **`git diff main --stat -- src/merge.rs` — zero diff — the highest-risk
    checkpoint in this packet**, per spec.md's own framing: the dedup
    comparison sits directly on top of `merge()`'s output, making it the
    most tempting place to fold comparison logic *into* `merge()` (e.g.
    "just have `merge()` take `last_published` and return `None` if
    unchanged"). Any nonzero diff here is a stop-and-flag condition, not a
    design call to resolve unilaterally.
  - `git diff main --stat` overall — confirm only `src/aggregator.rs`
    touched since Phase 3.
- Done when:
  - The measured duplicate rate is acted on correctly per the ~30%
    threshold (or an explicit, documented call if the number landed close
    to it), whichever direction it went, and `src/merge.rs` is confirmed
    unchanged.
- Commit boundary: `src/aggregator.rs` alone. Reverting it returns to
  Phase 2's measure-only state — histograms and counter keep working,
  nothing acts on the number — a one-line revert either direction, per
  spec.md's own Rollback Plan.

## Phase 5: load test at 100/500/1000 subscribers — `src/bin/loadtest.rs`, `Cargo.toml` (only if a new dependency is needed)

### 7. Build the `loadtest` binary

- Files or areas: `src/bin/loadtest.rs` (new); `Cargo.toml` only if a
  genuinely new dependency is required.
- Change:
  - A `clap`-derived CLI matching `src/bin/client.rs`'s existing pattern:
    `--addr` (gRPC server address, e.g. `http://127.0.0.1:50051`),
    `--clients` (subscriber count), `--duration-secs`.
  - Spawn `--clients` independent `tokio::spawn`ed tasks (collected in a
    `JoinSet` or `Vec<JoinHandle<u64>>`), each connecting via
    `orderbook_aggregator_client::OrderbookAggregatorClient::connect`
    (reusing the same generated client `src/bin/client.rs` already uses —
    no hand-rolled protocol handling), subscribing to `BookSummary(Empty
    {})`, and counting every message received off the stream without
    printing or rendering it.
  - The binary does **not** start its own server — it connects to an
    already-running `--addr`, per spec.md's explicit Invariant (this
    measures a real, independently-running server under real production
    load, not an in-process shortcut).
  - After `--duration-secs`, cancel/stop all client tasks, sum their
    per-client counts, and print the aggregate receive rate
    (`total_messages / duration_secs`) to stdout, then exit.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build` — clean.
  - No `cargo test` coverage expected — matching `src/bin/client.rs`'s
    standing "no tests for a manual-verification demo/tooling binary"
    convention, and spec.md's own Testing Strategy explicitly calling this
    piece "not a `cargo test`."
  - `git diff main --stat -- src/merge.rs` — zero diff (this task touches
    no aggregator or model code; cheap to reconfirm).
- Done when:
  - `cargo run --release --bin loadtest -- --addr http://127.0.0.1:50051
    --clients 5 --duration-secs 5` (a small smoke run against a locally
    running `aggregator`) connects, receives real messages, and prints a
    plausible aggregate rate — confirming the binary works before the real
    100/500/1000 runs in Task 8.

### 8. Run the load test at 100, 500, and 1000 subscribers, sampling CPU

- Files or areas: none (verification-only task; no source change).
- Change: none.
- Verification:
  - With the real `aggregator` binary already running (via `docker compose
    up --build` if a Docker daemon is available in this environment, or
    `cargo run --release -- --pair ethbtc --port 50051` otherwise — state
    explicitly which was used), run, in sequence:
    - `cargo run --release --bin loadtest -- --addr http://127.0.0.1:50051
      --clients 100 --duration-secs 60`
    - the same with `--clients 500`
    - the same with `--clients 1000`
  - During each run, sample `docker stats <container>` (if run under
    Docker) or the local process's CPU% via the OS's own tool (e.g.
    `ps`/`top` on the running `aggregator` PID, if not run under Docker —
    state which was used) at least twice, and record the aggregate
    receive-rate figure `loadtest` prints at the end of each run.
  - Report all three (CPU%, sustained receive-rate) pairs verbatim. If the
    shape looks non-linear (e.g. a knee well below 1000 subscribers), flag
    it plainly rather than smoothing it into a "linear, as expected"
    narrative — per spec.md's own framing, that would be the more
    interesting result.
  - If no Docker daemon and no way to sample CPU is available in this
    environment, report "not verified here" explicitly for this task rather
    than omitting it silently — matching this project's standing rule for
    an unavailable environment capability.
  - `git diff main --stat` overall — confirm only `src/bin/loadtest.rs`
    (and `Cargo.toml`, only if genuinely touched in Task 7) changed since
    Phase 4.
- Done when:
  - Three real (CPU%, receive-rate) measurements at 100/500/1000
    subscribers are recorded (or explicitly reported as unverifiable in
    this environment), ready to replace the README's current estimate in
    Phase 6.
- Commit boundary: `src/bin/loadtest.rs` (plus `Cargo.toml` if touched)
  alone. Reverting it has no effect on the `aggregator`/`client` binaries or
  any earlier phase's work.

## Phase 6: README — prediction vs. result, all numbers, production notes — `README.md`

### 9. Write the "Measurement" section and retire the old estimate

- Files or areas: `README.md` only.
- Change:
  - Add (or extend the existing "Behaviour under load" subsection) a
    "Measurement" section containing:
    - The prediction, stated faithfully (5-25µs p50, parse dominant, and
      why) — quoted or paraphrased from spec.md's Prediction section.
    - The measured p50/p99/p99.9 for total, parse, and merge+publish spans
      from Phase 2/3's real runs, plus the specific parse-as-percentage-of
      -total-p50 figure the prediction named — not just the total.
    - The gap between prediction and result, stated plainly (confirmed, or
      the more interesting outcome if it wasn't) — not silently reconciled.
    - The measured duplicate rate and Phase 4's decision (skip implemented,
      or not, and why, including the exact percentage).
    - The release-profile before/after p50 and before/after release build
      time (Task 5's four numbers).
    - The load-test table: (CPU%, sustained receive-rate) for 100, 500, and
      1000 subscribers (Task 8's three pairs), or the explicit "not verified
      here" note if Task 8 couldn't run in this environment.
    - An explicit statement of what ingest-to-publish latency is **not**:
      wire latency against the exchange — Binance's stream carries no event
      time, so only one venue could ever be compared against it, and
      cross-venue wire-latency comparison would be misleading.
  - Replace, don't leave standing alongside the new numbers, the two
    specific existing lines this section supersedes: the "roughly...low
    thousands of subscribers" estimate, and the "dedup deferred pending a
    measurement" line (both already present per spec.md's Current State).
  - Word budget: keep the README close to its pre-packet length; trimming
    elsewhere is allowed to make room, but no measured number and no part
    of the prediction/result comparison is cut to make the budget.
- Verification:
  - `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D
    warnings`, `cargo fmt --check` — full gate, clean, at the tip of
    everything landed through Phase 5 (documentation-only phase, but this
    project's standing convention runs the full gate at every
    README-touching phase regardless).
  - `wc -w README.md` — report the actual observed word count before and
    after this task, and confirm it stays close to the pre-packet baseline.
  - Read the finished "Measurement" section back against Tasks 4/5/6/8's
    actual recorded numbers, line by line — confirm every number in the
    README matches what was actually observed, not a rounded or
    reconstructed approximation.
  - `git diff main --stat -- src/merge.rs` — zero diff (this task touches
    no source; sanity check nothing from Phases 2-5 slipped through
    unnoticed).
  - `git diff main --stat` overall — confirm the whole branch through this
    task touches only `Cargo.toml`, `src/model.rs`,
    `src/exchange/binance.rs`, `src/exchange/bitstamp.rs`,
    `src/aggregator.rs`, `src/bin/loadtest.rs`, `README.md`, and
    `specs/011-measurement/` — no other path, and specifically zero diff on
    `src/merge.rs`, `src/server.rs`, and `proto/orderbook.proto`.
- Done when:
  - A reader of the README alone sees the prediction, the real measured
    result, and the honest gap between them, without needing to read the
    spec packet to reconstruct what was measured.
- Commit boundary: `README.md` alone — matching this project's standing
  pattern (its `009-resilience` and `007-merge` equivalents) of landing a
  doc pass as its own, separately revertible commit.

## Phase 7: start the 24-hour run — no code change, a running process

### 10. Start the run against the Phase 6 tip and record the start time

- Files or areas: `README.md` only, for the start-time note (a second, small
  commit on top of Task 9's doc pass, not folded into it).
- Change: none in `src/`. Operationally: start the real `aggregator` build
  (`docker compose up`, matching how the service is actually deployed, or
  its closest available equivalent in this environment) against a real pair
  (`ethbtc`) and leave it running for 24 hours.
- Verification:
  - Confirm the running build is built from the Task 9 (Phase 6) tip —
    record `git rev-parse HEAD` alongside the run's start time.
  - Add the run's start time (and the commit it was built from) to
    `README.md` as its own small addition.
  - If Docker (or an equivalent way to run a genuine 24-hour unattended
    process) is unavailable in this environment, state that explicitly in
    the README rather than reporting a run that never actually started.
- Done when:
  - The run's start time and source commit are recorded in `README.md`, or
    its unavailability in this environment is stated plainly — not a
    description of a run that hasn't actually started.
- Commit boundary: a small `README.md` commit recording the start time.

### 11. Write up the 24-hour run, or mark it explicitly pending

- Files or areas: `README.md` only.
- Change:
  - **If the run has completed by the time this task runs:** add, to the
    same README section, the reconnect count per venue (grepped from
    `src/feed.rs`'s existing `"connected"`/`"reconnecting after backoff"`
    log lines), whether a reconnect lines up with Binance's documented ~24h
    forced-close boundary, p50/p99 at the start of the run vs. the end
    (drift would indicate a leak or something accumulating), peak RSS
    (sampled via `docker stats` or `/proc/<pid>/status`'s `VmRSS` — state
    which was used), how many times a venue was excluded as stale and for
    how long (from `src/aggregator.rs`'s existing staleness path), and the
    full-24h duplicate rate compared against the short local-workload rate
    from Task 6/9.
  - **If the run has not completed by the time this packet is otherwise
    ready for review:** state plainly, in the same README section, that the
    write-up is pending, with the run's start time and expected completion
    — never presented as done before it is, per spec.md's explicit
    Invariant.
- Verification:
  - Read the README section back and confirm it states one of the two
    outcomes above unambiguously — no number from an incomplete run
    presented as a final figure.
  - `git diff main --stat -- src/merge.rs` — zero diff (final confirmation;
    no code changes in this phase at all).
- Done when:
  - The README honestly reflects whether the full 24-hour write-up is in or
    still pending, matching what was actually observed, not what was hoped
    for.
- Commit boundary: a second small `README.md` commit, separate from Task
  10's start-time commit, landing only once the run's outcome (complete or
  still pending) is known. Neither this nor Task 10's commit has any effect
  on runtime code — reverting either only removes documentation.

## Final Verification

Before closing the packet, run the following once at the tip of the
branch — the most representative real-behaviour path for this step (a real
instrumented run, a real profile-change comparison, a real multi-subscriber
load test), not a rerun of the per-task checks alone:

- `cargo build && cargo test && cargo clippy --all-targets -- -D warnings &&
  cargo fmt --check` — all clean; report the actual final test count
  (32-test baseline plus the four Phase 2 tests plus, conditionally, Phase
  4's one test).
- `cargo run --release -- --pair ethbtc --port 50051` with `RUST_LOG=info`,
  live against real or proxied connectivity for at least a few minutes:
  quote an actual periodic log line showing non-degenerate p50/p99/p99.9
  for total/parse/merge+publish and a duplicate percentage.
- `cargo run --release --bin loadtest -- --addr http://127.0.0.1:50051
  --clients 100 --duration-secs 30` (a real load-test run against the real
  server above) — confirm it prints a plausible aggregate receive rate and
  the server keeps running afterward.
- `hdrhistogram` confirmed as a `[dependencies]` entry via `grep
  hdrhistogram Cargo.toml`.
- `git diff main --stat -- src/merge.rs` — zero diff, final confirmation.
- `git diff main --stat` at the tip — confirm only the files named in Task
  9's scope list (plus the two small README commits from Phase 7) are
  touched — no other path, and specifically zero diff on `src/merge.rs`,
  `src/server.rs`, and `proto/orderbook.proto`.
- `wc -w README.md` — confirm it stays close to the pre-packet baseline.
- The README's "Measurement" section, read once more end to end, states the
  prediction, the measured result (including the parse-vs-rest percentage),
  the gap between them, the dedup decision and its number, the
  release-profile before/after, the load-test table, and the 24-hour run's
  status (complete or explicitly pending) — all present, none reconstructed
  from memory rather than the actual recorded numbers.
