---
spec_name: "Reset latency histograms per report tick, not lifetime-cumulative"
spec_id: "013"
spec_folder: "013-rolling-histogram"
status: "approved"
created_at: "2026-08-28"
updated_at: "2026-08-28"
created_by: "one-shot-spec-packet"
creation_mode: "one-shot-minor-update"
goal: "Make each ~30s periodic latency report in src/aggregator.rs describe a recent (~30s) window instead of the process's entire lifetime, by resetting each of the three hdrhistogram::Histogram<u64> fields immediately after it is read for the log line."
purpose: "The three histograms are created once in Aggregator::new() and never reset, so every logged p50/p99/p99.9 is cumulative since process start. Confirmed live during the 24-hour run started 2026-08-26: after 32+ hours, total_p999_us still reported a single ~412ms outlier from early in the run. A 30-second periodic report line that silently means 'worst ever' rather than 'worst recently' is misleading and was already flagged as a known, unfixed gap in README.md's Measurement section."
source_inputs:
  - "inputs/001-human-brief.md"
source_agents: []
parent_request: "specs/013-rolling-histogram/inputs/001-human-brief.md, 2026-08-28"
related_paths:
  - "src/aggregator.rs"
  - "README.md"
verification_level: "real-behavior"
complexity: "small"
---

# Spec: 013-rolling-histogram

## Problem

`src/aggregator.rs`'s `Aggregator` holds three `hdrhistogram::Histogram<u64>`
fields (`parse_histogram`, `merge_publish_histogram`, `total_histogram`),
constructed once in `Aggregator::new()`. `record_duration()` adds a sample to
the relevant histogram on every parsed/published book, and `log_report()`
reads `value_at_quantile()` off those same histograms every `REPORT_INTERVAL`
(30s) — but nothing ever resets them. Every periodic log line therefore
reports percentiles over the process's entire runtime, not a recent window,
which silently contradicts what a "logged every 30s" line reads as to
anyone watching it.

This was found by observation during 011-measurement's 24-hour live run, not
predicted in that spec — the simplest working implementation (one histogram,
keep feeding it) shipped without anyone flagging the cumulative-vs-windowed
distinction at the time.

## Goal

Reset each of the three histograms immediately after `log_report()` reads it,
so the next `REPORT_INTERVAL` window starts clean and every subsequent log
line genuinely describes "the last ~30s," not "since the process started."

## Purpose

A 24-hour or longer run is the realistic deployment shape this project is
built for (see README.md's Measurement section). A cumulative histogram makes
the periodic log line actively misleading for its stated purpose — spotting a
regression that starts *now* — because one bad tick early in the run
dominates every later percentile indefinitely. This is a targeted, low-risk
fix to already-shipped instrumentation code, not new instrumentation.

## Scope

In scope:

- `Aggregator::log_report` (or its caller) in `src/aggregator.rs`: reset
  `parse_histogram`, `merge_publish_histogram`, and `total_histogram`
  immediately after each is read for the periodic log line.
- A real regression test proving the reset actually happens: record a
  sample, read/report it, record a second, distinctly different sample,
  and assert the second report's percentiles reflect only the second
  sample, not the first.
- `README.md`'s Measurement section: remove the "**Known gap, not yet
  fixed**" paragraph and replace it with an accurate description of the
  new rolling ~30s-window semantics.

Out of scope (do not build unless something concrete forces it during
implementation):

- A dual-tracking scheme (a lifetime histogram alongside a windowed one).
- A sliding window with sub-buckets, decay, or any multi-window design.
- Any change to `update_rate_per_sec`'s calculation — it already correctly
  measures a window via `update_count_at_last_report`; only the histogram
  read/reset semantics change.
- Any change to `Aggregator`'s public surface, `run()`'s `select!` shape
  beyond the reset call itself, or any other file's behavior.

A single reset-per-report-tick window is the simplest fix that matches what
README.md already promised ("a rolling-window fix is planned") and this
project's stated preference for measuring the actual thing before adding
sophistication — not a design compromise, the intended outcome.

## Current State

Verified by reading `src/aggregator.rs` in full:

- `Aggregator::new()` (around line 94) constructs all three histograms once,
  each via `Histogram::new(HISTOGRAM_SIGFIG)` (3 significant figures), and
  never resets them anywhere in the file.
- `record_duration()` (around line 268) records a duration in nanoseconds
  into a `&mut Histogram<u64>`, logging (not failing) on the rare
  `hdrhistogram` rejection case.
- `log_report()` (around line 279) takes `&Aggregator` (immutable) and reads
  `value_at_quantile(0.50/0.99/0.999)` off all three histograms plus computes
  `update_rate_per_sec` (correctly windowed already, via
  `update_count_at_last_report` passed in by the caller) and `duplicate_pct`
  (a running, not windowed, cumulative percentage — out of scope here, not
  part of the brief).
- `run()`'s `tokio::select!` (around line 165) calls
  `log_report(&aggregator, update_count_at_last_report, REPORT_INTERVAL)` on
  every `report_tick.tick()`, then updates
  `update_count_at_last_report = aggregator.update_count`. `log_report`
  currently borrows `&aggregator` immutably; making it reset the histograms
  requires either taking `&mut Aggregator` or having the caller (`run()`)
  perform the reset itself immediately after the call returns.
- The resolved dependency version, confirmed via `Cargo.lock`, is
  `hdrhistogram 7.6.0`. Its `Histogram::reset(&mut self)` (confirmed by
  reading the crate source at
  `~/.cargo/registry/src/.../hdrhistogram-7.6.0/src/lib.rs`, not assumed)
  calls `self.clear()` (zeroes every bucket count and `total_count`) and
  resets the tracked min/max back to their original sentinel values, while
  preserving the histogram's configuration (significant figures,
  auto-resize setting) — no reallocation, no change to what values it can
  represent afterward. This is exactly the "clear stats, keep config"
  behavior the fix needs; nothing about its signature or cost forces a
  fancier design.
- README.md's Measurement section (verified by reading it) currently states,
  verbatim: "**Known gap, not yet fixed**: the histogram is never reset, so
  every logged percentile is cumulative since process start, not a recent
  window — one bad tick an hour into a run still dominates p999 a day
  later. A rolling-window fix is planned; until it lands, read p999
  especially as 'worst moment ever,' not 'worst moment recently.'" This
  paragraph must be replaced, not merely trimmed, once the fix lands.

## Invariants and Critical Don'ts

- **`src/merge.rs` must not change.** This is this project's standing,
  most-emphasized invariant across every prior packet. Nothing about this
  fix has any reason to touch it — the histograms live entirely in
  `src/aggregator.rs`, wrapping the existing `merge::merge()` call site, not
  inside it. Verified at completion via `git diff main --stat -- src/merge.rs`
  showing zero diff.
- **`update_rate_per_sec`'s calculation does not change.** It already
  correctly measures a window via `update_count_at_last_report`; only the
  histogram read/reset semantics are in scope.
- **No fancier multi-window/dual-tracking scheme** unless a real, concrete
  reason surfaces during implementation — and if it does, that's a reason
  to pause and update this packet, not to quietly build past its stated
  scope.
- **The reset must happen only after the value has been read and logged**,
  never before — resetting first would report an empty/degenerate window
  every tick instead of the just-elapsed one.

## Acceptance Criteria

- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all pass.
- A real regression test in `src/aggregator.rs`'s `mod tests` proves the
  reset behavior: record a sample into a histogram, read/report it (or call
  whatever function performs the read-then-reset), record a second,
  distinctly different sample, and assert the second report's percentiles
  reflect only the second sample — not a blend with, or dominance by, the
  first. This is the concrete bug this packet fixes; a passing build alone
  is not sufficient evidence.
- `git diff main --stat -- src/merge.rs` shows zero diff.
- README.md's Measurement section accurately describes the new rolling
  ~30s-window semantics, with the "Known gap, not yet fixed" language fully
  removed (not left standing alongside the new text).
- Ideally, verified against a real short live run in addition to the unit
  test: confirm a log line's p999 actually drops after an intervening quiet
  window, the concrete real-world behavior this fix exists to produce. If a
  live/proxied connection isn't available in the execution environment,
  state that explicitly rather than silently skipping it.

## Risks and Tradeoffs

- **Low risk overall** — this is a targeted change to already-tested,
  already-instrumented code with no change to program structure, public
  surface, or the merge/publish path itself.
- **Losing visibility into a true worst-case-ever outlier.** A reset-per-tick
  window means a single catastrophic latency spike is only visible in the
  one report window it occurred in, then never again — trading "worst ever"
  for "worst recently," exactly as the brief asks for. Worth stating plainly
  in the README rather than silently, since it is a genuine, deliberate
  tradeoff, not a free improvement.
- **Signature choice for `log_report`.** Changing it from `&Aggregator` to
  `&mut Aggregator` (or performing the reset in the caller instead) is an
  implementation-level decision with no behavioral difference either way;
  left open for whichever reads most naturally against the existing `run()`
  call site once written, not pre-decided here.

## Testing Strategy

- Unit test in `src/aggregator.rs`'s `mod tests`, following this file's
  existing pattern (hand-constructed `Aggregator`/histogram state, no
  `Instant::now()`/real clock dependency where avoidable) — the two-sample,
  read-reset-read shape described in Acceptance Criteria above. Name it as a
  behavior sentence (e.g.
  `a_histogram_reset_after_report_excludes_prior_samples`), per this
  project's naming convention, and give it a comment stating the bug it
  regresses against (lifetime-cumulative percentiles hiding a recent
  quiet window).
- Existing test suite must continue to pass unedited — this is a targeted
  addition, not a refactor of anything `record_and_publish`,
  `fresh_venues`, or `past_grace` currently do.
- `git diff main --stat -- src/merge.rs` — zero diff, checked at completion.
- Optional but preferred: a short real/proxied live run (`cargo run --
  --pair ethbtc --port 50051`, `RUST_LOG=info`) spanning at least two report
  ticks with a deliberately induced quiet gap between them, confirming the
  second log line's percentiles do not still reflect activity from the
  first window.

## Rollback Plan

A one-file, one-behavior change. Reverting the `src/aggregator.rs` commit
returns to the current lifetime-cumulative behavior with no other effect —
no schema, no public API, no cross-file dependency introduced. If the README
commit lands separately, reverting it alone simply restores the "known gap"
language, independent of whether the code fix is kept.

## Open Questions

None outstanding — the fix, its scope, and the crate API it depends on
(`hdrhistogram::Histogram::reset(&mut self)`, confirmed against the resolved
7.6.0 source) are all settled by this spec.
