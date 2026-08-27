# Tasks: 013-rolling-histogram

## Task Writing Rules

- Each task should describe a real unit of progress.
- Each task should name the expected files or areas touched.
- Each task should include explicit verification.
- Prefer behavior-level verification over mock-only checks.

Two phases below map 1:1 to plan.md's own phase breakdown.

**The one check that repeats across this entire packet:**
`git diff main --stat -- src/merge.rs` must show zero diff. This project's
merge logic is required to stay pure, and this fix has no legitimate reason
to touch it — the check is run at the end of every task, not just once at
the end.

No dual-tracking (lifetime + windowed) histogram scheme, no sliding window
with sub-buckets, and no change to `update_rate_per_sec`'s calculation may
appear in any task below. If implementation drifts toward one, stop and flag
it rather than treating it as a natural extension.

## Phase 1: reset each histogram after it's read for the report — `src/aggregator.rs`

### 1. Reset `parse_histogram`, `merge_publish_histogram`, and `total_histogram` immediately after each is read in `log_report`

- Files or areas: `src/aggregator.rs` only.
- Change:
  - Give `log_report` (or its caller in `run()`) mutable access to
    `Aggregator`'s three histogram fields. Whichever shape reads more
    naturally against the existing call site is fine — e.g. changing
    `log_report`'s signature from `&Aggregator` to `&mut Aggregator`, or
    having `run()` call `.reset()` on each histogram immediately after the
    existing `log_report(&aggregator, ...)` call returns. Either way, the
    reset must happen strictly after the three `value_at_quantile` reads
    for that tick, never before.
  - Call `.reset()` on `parse_histogram`, `merge_publish_histogram`, and
    `total_histogram` right after their values are captured for the log
    line — `hdrhistogram::Histogram::reset(&mut self)` (confirmed against
    the resolved `7.6.0` source, not assumed) clears all bucket counts and
    the tracked min/max while preserving significant-figure configuration,
    so nothing else about the histograms' behavior changes across the
    reset.
  - `update_rate_per_sec` and `duplicate_pct`'s calculations in
    `log_report` are untouched — this task changes only how the three
    histograms are read/reset, nothing else in the function body.
  - Update the doc comments on the three histogram fields (currently on
    `Aggregator`'s struct definition) and on `log_report` itself to state
    the new contract explicitly: each histogram is reset immediately after
    its periodic report, so it describes the last `REPORT_INTERVAL`
    (~30s), not the process's lifetime. The prior comments describe the
    fields without saying whether they're windowed or cumulative — that
    silence is part of why this bug shipped unnoticed, so it should not
    persist once fixed.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build` — clean.
  - `cargo test` — a new test in `src/aggregator.rs`'s `mod tests`, named
    as a behavior sentence (e.g.
    `a_histogram_reset_after_report_excludes_prior_samples`), carrying a
    comment stating the bug it regresses against (lifetime-cumulative
    percentiles hiding a recent quiet window, confirmed live during
    011-measurement's 24-hour run). Shape: record one sample with a known,
    large duration into a histogram (directly, or by driving
    `record_and_publish` once with a hand-constructed `Book` — following
    this file's existing "pass timestamps in by hand" pattern rather than
    calling `Instant::now()` inside the function under test where
    avoidable); call whatever function now performs the read-then-reset
    for that histogram and capture the reported value; record a second
    sample with a known, distinctly smaller (or otherwise clearly
    different) duration; call the read-then-reset function again and
    capture the second reported value; assert the second value reflects
    only the second sample — e.g. if the first sample was large and the
    second small, assert the second report's value is close to the second
    sample's duration, not still elevated by the first.
  - Existing 32-test baseline continues to pass unedited — confirmed by
    reading the current `mod tests` before this task: no existing
    assertion reads a `value_at_quantile` result (they assert on
    `.len()`/counts), so none of them can be affected by a reset that only
    happens inside `log_report`'s own call path.
  - **`git diff main --stat -- src/merge.rs` — zero diff.**
  - `git diff main --stat` overall — confirm only `src/aggregator.rs`
    touched since `main` (README follows in Task 2, as its own commit).
  - Manual/live, preferred but not blocking: `cargo run -- --pair ethbtc
    --port 50051` with `RUST_LOG=info` against real or proxied
    connectivity, spanning at least two `REPORT_INTERVAL` (30s) ticks with
    a deliberately quiet or lower-activity gap between them. Quote the two
    actual log lines and confirm the second's p999 does not still reflect
    an outlier that was only present in the first window. If no
    live/proxied connection is available in this environment, state that
    explicitly rather than silently skipping it.
- Done when:
  - The new regression test passes by name, the existing 32-test baseline
    passes unedited, `src/merge.rs` shows zero diff, and (where possible) a
    real live run confirms the second report window's percentiles do not
    carry forward stale numbers from the first.
- Commit boundary: this task's `src/aggregator.rs` change and its new test
  land together as one commit. Reverting it returns to the current
  lifetime-cumulative behavior — buildable and passing, but a real
  regression against this packet's goal.

## Phase 2: README — describe rolling-window semantics, remove the known-gap language — `README.md`

### 2. Update README.md's Measurement section

- Files or areas: `README.md` only.
- Change:
  - Remove, in full, the existing paragraph: "**Known gap, not yet
    fixed**: the histogram is never reset, so every logged percentile is
    cumulative since process start, not a recent window — one bad tick an
    hour into a run still dominates p999 a day later. A rolling-window fix
    is planned; until it lands, read p999 especially as 'worst moment
    ever,' not 'worst moment recently.'"
  - Replace it with an accurate description: each histogram now resets
    immediately after its periodic ~30s report, so p50/p99/p99.9 describe
    the last ~30s window, not the process's lifetime. State the tradeoff
    plainly — a genuine worst-ever outlier is now visible only in the one
    report window it occurred in, not indefinitely afterward, which is the
    intended behavior change, not an incidental side effect.
  - If Task 1's live-run check produced a concrete before/after example
    (an outlier visible in one report line and gone from the next),
    include it as real observed evidence, matching this project's existing
    convention of reporting actually-observed numbers over abstract
    description alone.
- Verification:
  - `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D
    warnings`, `cargo fmt --check` — full gate, clean (documentation-only
    task; this project's standing convention runs the full gate at every
    README-touching task regardless).
  - `grep -n "Known gap" README.md` — returns nothing, confirming the old
    language is fully removed, not left standing alongside the new text.
  - Read the replacement paragraph back and confirm it states the ~30s
    rolling-window semantics and the worst-ever-visibility tradeoff
    explicitly.
  - `git diff main --stat -- src/merge.rs` — zero diff (no source touched
    in this task; final confirmation nothing slipped through).
  - `git diff main --stat` overall — confirm the whole branch through this
    task touches only `src/aggregator.rs`, `README.md`, and
    `specs/013-rolling-histogram/` — no other path.
- Done when:
  - A reader of README.md alone understands the histograms are windowed
    (~30s), not lifetime-cumulative, with no "Known gap" language
    remaining anywhere in the file.
- Commit boundary: `README.md` alone — a separately revertible doc-pass
  commit, matching this project's standing pattern.

## Final Verification

Before closing the packet, run once at the tip of the branch:

- `cargo build && cargo test && cargo clippy --all-targets -- -D warnings
  && cargo fmt --check` — all clean; report the actual final test count
  (32-test baseline plus the one new regression test from Task 1).
- `git diff main --stat -- src/merge.rs` — zero diff, final confirmation.
- `git diff main --stat` at the tip — confirm only `src/aggregator.rs`,
  `README.md`, and `specs/013-rolling-histogram/` are touched — no other
  path.
- `grep -n "Known gap" README.md` — returns nothing.
- Report whether the live-run confirmation (a log line's p999 dropping
  after an intervening quiet window) was actually performed in this
  environment, and quote the observed log lines if so — or state plainly
  that it was not verified here.
- Merge to `main` with `--no-ff`, per the brief's explicit instruction that
  this work lands directly on `main`, not a research branch.
