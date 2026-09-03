# The latency histograms are lifetime-cumulative, not rolling — fix it

## What's wrong, verified by reading the code

`src/aggregator.rs`'s three `hdrhistogram::Histogram<u64>` fields
(`parse_histogram`, `merge_publish_histogram`, `total_histogram`) are
created once in `Aggregator::new()` and every `record_duration()` call
adds to them for the lifetime of the process — there is no `.reset()` or
equivalent anywhere. `log_report()` reads `value_at_quantile()` off those
same ever-growing histograms every ~30s tick.

Consequence, confirmed live during the 24-hour run started 2026-08-26 for
`012-kraken`/`011-measurement`: after 32+ hours, `total_p999_us` was still
reporting a single ~412ms outlier from early in the run, hours after it
happened. Every 30s log line's p50/p99/p99.9 describes "worst/median since
process start," not "worst/median recently" — which is not what a
30-second periodic report line reads as, and not useful for spotting a
regression that starts happening *now*.

This was found by observation, not planned — `011-measurement`'s spec
never discussed cumulative vs. windowed semantics; the simplest
implementation (one histogram, keep feeding it) shipped without anyone
flagging the difference. README.md's Measurement section currently
documents this as a known, unfixed gap:

> **Known gap, not yet fixed**: the histogram is never reset, so every
> logged percentile is cumulative since process start, not a recent
> window — one bad tick an hour into a run still dominates p999 a day
> later. A rolling-window fix is planned; until it lands, read p999
> especially as "worst moment ever," not "worst moment recently."

## What to do

Make the periodic report describe a recent window, not the process's
entire lifetime. The obvious, simplest fix matching this project's stated
preference for boring and direct over clever: reset each histogram
immediately after it's read for the periodic log line, so the next ~30s
window starts clean. `hdrhistogram::Histogram` has a `.reset()` method —
confirm its exact behavior/cost against the resolved crate version on
docs.rs before relying on it, don't assume.

Do not build a fancier rolling/decaying/multi-window scheme (e.g. keeping
both a lifetime histogram and a windowed one, or a sliding window with
sub-buckets) unless a real reason surfaces during implementation — a
single reset-per-report-tick window is what the README already promised
and is consistent with "measure the actual thing you're trying to show
before adding sophistication."

## Constraints

- `src/merge.rs` must not change — this project's standing invariant,
  checked via `git diff main --stat -- src/merge.rs` at completion, same
  as every prior packet.
- The `update_rate_per_sec` calculation already correctly measures a
  window (via `update_count_at_last_report`) — don't disturb that logic,
  only the histogram read/reset semantics.
- Keep the existing test
  `a_published_book_records_both_parse_and_merge_publish_samples`-style
  tests passing, and add real regression coverage for the reset behavior
  itself: record a sample, read/report it, record a second sample after a
  simulated reset, and assert the second report doesn't still reflect the
  first sample's value (the concrete bug this packet fixes).
- Update README.md's Measurement section: remove the "known gap, not yet
  fixed" language once the fix lands, and describe what the numbers now
  mean (a rolling ~30s window) instead.
- This work lands directly on `main` (not a research branch) — a small,
  real fix to already-shipped code, following this project's normal
  packet process: branch `013-rolling-histogram`, spec → plan → tasks,
  then implement, then merge with `--no-ff`.

## Context: a related small fix landed just before this, for orientation

Immediately before this packet, `012-kraken` (adding Kraken as a third
venue) was merged into `main`. A follow-up gap was found and fixed
directly on `main` afterward: `src/bin/client.rs`'s `VENUES` constant
(used for the terminal client's per-venue status header row) still only
listed `[Venue::Binance, Venue::Bitstamp]` after the merge — the book rows
already rendered Kraken's levels correctly (no venue filtering there), but
the header's live/stale indicator never grew a third entry. Fixed as a
one-line change (`[Venue; 3]` with `Venue::Kraken` appended), tracked in
`specs/012-kraken/revisions.md` entry 4. Unrelated to this packet's actual
scope (the histogram fix) — noted here only so this packet's Current State
reflects the actual, current state of `main` rather than a stale snapshot
from before that fix landed.

## Acceptance

- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D
  warnings`, `cargo fmt --check` all pass.
- A real regression test proves the reset behavior, not just that the
  code compiles.
- `git diff main --stat -- src/merge.rs` shows zero diff.
- README.md's Measurement section accurately describes rolling-window
  semantics, with the "known gap" language removed.
- Ideally verified against a real live run (even a short one), not just
  unit tests — confirm a log line's p999 actually drops after an
  intervening quiet window, the concrete behavior this fix is for.
