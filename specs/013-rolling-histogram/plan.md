# Plan: 013-rolling-histogram

## Summary

One phase. This is a small, localized fix to already-shipped instrumentation
in a single file, plus a documentation update — it does not warrant multi-
phase sequencing. The code change and its regression test land together
(the test would be meaningless without the fix, and the fix is unverified
without the test); the README update lands as its own commit immediately
after, matching this project's standing pattern of a separate, independently
revertible doc-pass commit (see e.g. `011-measurement` Phase 6,
`009-resilience` Phase 6).

## Phase Breakdown

### Phase 1: reset each histogram after it's read for the report — `src/aggregator.rs`

- Objective: `log_report()` (or its caller) resets `parse_histogram`,
  `merge_publish_histogram`, and `total_histogram` immediately after each is
  read for the periodic log line, so the next `REPORT_INTERVAL` window
  starts clean. No other behavior in `run()`'s loop, `record_and_publish()`,
  `fresh_venues()`, or `past_grace()` changes.
- Main changes:
  - `src/aggregator.rs`: give `log_report` (or whichever function ends up
    performing the reset — a `&mut Aggregator` signature on `log_report`
    itself, or the reset performed by `run()` immediately after the
    existing `log_report(&aggregator, ...)` call returns, whichever reads
    more naturally against the existing call site) mutable access to the
    three histograms, and call `.reset()` on each right after its three
    `value_at_quantile` reads are captured. `update_rate_per_sec` and
    `duplicate_pct`'s calculations are untouched.
  - Doc comments on the three histogram fields and on `log_report` updated
    to state the new "reset every report tick" contract — the existing
    comments currently describe them without saying whether they're
    windowed or cumulative; that ambiguity is exactly what caused this bug
    to ship unnoticed, so it should not persist once fixed.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build` — clean.
  - `cargo test` — a new test,
    `a_histogram_reset_after_report_excludes_prior_samples` (or an
    equivalently named behavior sentence), in `src/aggregator.rs`'s `mod
    tests`: record one sample into a histogram (or drive `Aggregator`
    through `record_and_publish` once), call whatever function performs the
    read-then-reset, record a second, distinctly different sample, call it
    again, and assert the second report's captured percentile values
    reflect only the second sample — not the first, and not some blend of
    both. This is the one test that actually proves the fix; nothing else
    in this phase is meaningful evidence without it.
  - Existing 32-test baseline continues to pass unedited — no existing
    assertion depends on cumulative histogram behavior (confirmed by
    reading the current `mod tests`: `a_published_book_records_both_parse_
    and_merge_publish_samples` and its neighbors assert on sample *counts*
    via `.len()`, not on read-out percentile values, so a reset happening
    after the read they never trigger does not affect them).
  - **`git diff main --stat -- src/merge.rs` — zero diff.** This packet
    touches only report-timing/histogram code around the existing
    `merge::merge()` call site, never the call itself; checked here as the
    single structural invariant every prior packet in this project checks
    at completion.
  - `git diff main --stat` overall — confirm only `src/aggregator.rs`
    touched in this phase (README follows in Phase 2, as its own commit).
  - Manual/live (preferred, not blocking): `cargo run -- --pair ethbtc
    --port 50051` with `RUST_LOG=info` against real or proxied connectivity,
    spanning at least two `REPORT_INTERVAL` (30s) ticks with a deliberately
    quiet gap between them (e.g. briefly interrupting the proxy or just
    observing a naturally quiet moment); confirm the second log line's
    p999 is lower than the first's if the first window contained a
    genuine outlier, or at minimum does not carry forward a stale earlier
    number once samples clearly differ between windows. If no live/proxied
    connection is available in the execution environment, state that
    explicitly rather than silently skipping this check.
- Done looks like: the periodic log line genuinely reports only the last
  ~30s of samples, proven by a passing regression test and (where possible)
  a live run, with `src/merge.rs` untouched and the rest of the existing
  test suite unedited.
- Commit boundary: this phase's `src/aggregator.rs` change and its new test
  land together as one commit. Reverting it returns to the current
  lifetime-cumulative behavior — a real regression against this packet's
  goal, but a buildable, already-tested, functioning state.

### Phase 2: README — describe rolling-window semantics, remove the known-gap language — `README.md`

- Objective: bring README.md's Measurement section in line with what the
  code now actually does, once Phase 1 has landed.
- Main changes:
  - `README.md`: replace the existing "**Known gap, not yet fixed**:
    the histogram is never reset..." paragraph with an accurate description
    of the new behavior — each histogram resets immediately after its
    periodic report, so p50/p99/p99.9 describe the last ~30s, not the
    process's lifetime — and state the tradeoff plainly (a true worst-ever
    outlier is now only visible in the one report window it occurred in,
    not indefinitely afterward).
  - If Phase 1's live-run check (or a dedicated one run here) produced a
    concrete before/after example (e.g. "an outlier at minute 3 no longer
    appears in minute 4's p999"), include that as real evidence alongside
    the description, matching this project's existing convention of
    reporting actually-observed numbers rather than only describing
    behavior abstractly.
- Verification:
  - `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D
    warnings`, `cargo fmt --check` — full gate, clean (documentation-only
    phase, but this project's standing convention runs the full gate at
    every README-touching phase regardless — see `011-measurement` Phase 6,
    `009-resilience` Phase 6).
  - Read the finished paragraph back and confirm the exact "Known gap, not
    yet fixed" sentence no longer appears anywhere in `README.md`
    (`grep -n "Known gap" README.md` returns nothing).
  - `git diff main --stat -- src/merge.rs` — zero diff (no source touched
    in this phase; final confirmation nothing slipped through).
  - `git diff main --stat` overall — confirm the whole branch through this
    phase touches only `src/aggregator.rs`, `README.md`, and
    `specs/013-rolling-histogram/` — no other path.
- Done looks like: a reader of README.md alone understands the histograms
  are windowed (~30s), not lifetime-cumulative, with no stale "known gap"
  language left standing anywhere in the file.
- Commit boundary: `README.md` alone — matching this project's standing
  pattern of landing a doc pass as its own, separately revertible commit.

## Cross-Cutting Considerations

- **`src/merge.rs` zero-diff, checked at the end of both phases**, not just
  claimed once at the end of the packet — same discipline every prior
  packet in this project has applied to this file.
- **No new public surface.** This fix does not add anything to
  `Aggregator`'s externally visible behavior (there is none — it's a
  task-local struct) and does not change `run()`'s function signature or
  the `watch` channel's payload type.
- **Untouched-files discipline.** Every file in this repository other than
  `src/aggregator.rs`, `README.md`, and `specs/013-rolling-histogram/`
  should show zero diff at the tip of this branch — in particular
  `src/merge.rs`, `src/server.rs`, `src/feed.rs`, `src/exchange/*.rs`,
  `src/main.rs`, `proto/orderbook.proto`. A phase whose diff unexpectedly
  touches any of those is a stop-and-flag condition, not a natural
  extension of scope.

## Verification Gates

Before this branch is considered ready to hand off (per the brief: it lands
directly on `main`, not a research branch — merge with `--no-ff` once these
gates pass):

- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all clean at the tip of the branch.
- `cargo test` reports the existing 32-test baseline plus the one new
  regression test from Phase 1, identifiable by its behavior-sentence name
  — report the actual observed count, not an arithmetic guess.
- `git diff main --stat -- src/merge.rs` shows zero diff.
- `grep -n "Known gap" README.md` returns nothing; the Measurement section
  accurately describes the rolling ~30s-window semantics.
- `git diff main --stat` at the tip shows only `src/aggregator.rs`,
  `README.md`, and `specs/013-rolling-histogram/` touched — no other path.
- Ideally, a real live-run confirmation that a log line's p999 actually
  drops after an intervening quiet window is reported (or explicitly
  marked "not verified here" if no live/proxied connection is available in
  the execution environment).

## Expected Drift Triggers

If any of the following becomes true while implementing, update spec.md
before continuing rather than improvising past it:

- Resetting the histograms turns out to require touching
  `update_rate_per_sec`'s calculation or `update_count_at_last_report`'s
  handling — spec.md explicitly scopes those out; if the two turn out to be
  more coupled than they currently appear, that's worth a documented call,
  not a silent expansion.
- A real reason surfaces during implementation to want a dual-tracking
  (lifetime + windowed) scheme instead of a plain reset — spec.md
  explicitly rules this out absent a concrete forcing reason; flag it
  rather than building it.
- `hdrhistogram::Histogram::reset()` turns out to behave differently at
  runtime than its source (`~/.cargo/registry/.../hdrhistogram-7.6.0/src/
  lib.rs`) suggests — e.g. an unexpected cost, or config not actually
  preserved — worth a documented finding, not a silent workaround.
- No live/proxied connection is available in the execution environment for
  the manual verification step in either phase — report "not verified
  here" explicitly, per this project's standing rule for an unavailable
  environment capability, rather than silently omitting the check.
