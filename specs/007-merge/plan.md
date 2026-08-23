# Plan: 007-merge

## Summary

Three phases, matching this step's `complexity: medium` scope (one code
file, one doc file — smaller than `006-bitstamp`'s five, closer to
`005-aggregator`'s five but with less surface area, so three is the right
size rather than padding to match either). The phase boundary is fixed by
spec.md's own explicit sequencing requirement, not a planning choice: the
README cut-down has to land as its own reviewable commit **before** any
line of `src/merge.rs` changes, so a reviewer can read the doc diff without
the algorithm diff tangled into it.

- Phase 1 is the README cut-down alone: mechanics sections (Layout,
  Configuration, gRPC server, Docker, Quick start) slashed, no code touched,
  landing at ~150 lines / under 1,200 words. Nothing about the merge is
  described yet, because it doesn't exist yet — that would be describing
  unshipped behaviour.
- Phase 2 is the algorithm: `summarise()` → `merge()`, `Side`, `merge_side()`,
  the eight named tests, in `src/merge.rs` alone. This is the step's actual
  deliverable and its only real risk (the `Peekable`/`min_by` shape spec.md
  flags as new to this codebase).
- Phase 3 is the second README pass — merge described, tie-break stated,
  crossed-book behaviour called out, the internal-type tradeoff added to
  production notes — plus the full verification gate run once at the tip.

One structural fact governs every phase, checked at each one rather than
only at the end, per the task instructions: **`src/aggregator.rs` must show
zero diff against `main` throughout this branch.** `merge()`'s signature was
already fixed in step 4 precisely so this step wouldn't ripple outward: the
map-of-borrowed-books shape (`&BTreeMap<Venue, &Book>`) is unchanged, only
the body of what reads it changes. A failure at this checkpoint at any
phase means the signature contract from step 4 didn't actually hold, and is
a stop-and-flag condition, not something to patch around by touching the
aggregator to "make it fit."

## Phase Breakdown

### Phase 1: README cut-down — `README.md` only

- Objective: Land the mechanics-to-design-decisions rebalance as its own
  commit, reviewable independently of any algorithm change, per spec.md's
  explicit sequencing requirement.
- Main changes: `README.md`, current 326 lines / 2,268 words, target ~150
  lines / under 1,200 words.
  - Cut: Layout (57 → ~15 lines, a tree with one line per file — module docs
    carry the rest), Configuration (51 → ~15, a table of variable/default/
    what-it-controls), gRPC server (54 → ~20, the `grpcurl` reflection
    one-liner plus a pointer to `proto/orderbook.proto`), Docker (35 → ~12,
    the `docker compose up` line, published port, proxy variables), Quick
    start (25 → ~10).
  - Grow: a new design-decisions section, two to four lines per point —
    `watch` over `broadcast`, merging sorted sides instead of sort-the-concat,
    `f64` newtypes over fixed-point (with the measurement that settled it),
    the tie-break rule (state it as a forward pointer here since the rule
    itself isn't shipped until Phase 2 — see the "not yet decided" note
    below), one `Exchange` trait kept synchronous, one process per pair.
  - The production section ("what would change for production") can grow
    slightly — schema criticisms and what wasn't built belong there.
  - **What this phase must NOT do:** describe `merge()`, the tie-break rule
    as an implemented fact, or crossed-book behaviour as shipped — none of
    that exists until Phase 2 lands. If the design-decisions section needs
    to mention the tie-break rule at all in this phase, phrase it as a
    stated intent/default, not as documentation of running code.
- Verification:
  - `wc -l README.md` and `wc -w README.md` — confirm ~150 lines and under
    1,200 words.
  - Read-through: every cut section still points somewhere a reader can
    find the mechanical detail (module doc comments, `config.rs`, the
    `.proto` file) rather than just deleting the information.
  - `git diff main --stat` — only `README.md` in the diff.
  - `git diff main --stat -- src/aggregator.rs` — zero diff (checkpoint 1
    of 3; nothing in this phase touches code at all, but confirm rather
    than assume).
- Done looks like: a standalone, reviewable doc diff — no code changed, word
  count and section balance both match spec.md's target.
- Commit boundary: `README.md` alone. Reverting it restores the current
  326-line README with no effect on build or test state.

### Phase 2: the merge algorithm — `src/merge.rs` only

- Objective: The step's actual deliverable — replace single-venue
  `summarise()` with the real two-book `merge()` spec.md's Design section
  specifies exactly (signature unchanged from step 4, `Side` enum,
  `merge_side()`'s peekable-cursor walk, the eight named tests). This is
  also where the branch's one genuine risk lives, per spec.md's Risks
  section: the `Peekable`/`min_by` shape is new to this codebase.
- Main changes: `src/merge.rs`.
  - `Side` enum (`Bid`, `Ask`) with `better()` (the `Ordering` rule — price
    first via `Side`-dependent direction, `.then()` amount-descending tie-
    break, identical amount rule on both sides) and `levels()` (which list
    of the `Book` to read). Enum, not a bool — spec.md is explicit this is
    non-negotiable (a bool parameter is silently invertible with no
    compile error).
  - `merge_side()`: one `Peekable` cursor per venue over the requested side,
    built from `venues.iter()` (preserves `BTreeMap` iteration order —
    this is the mechanism the required comment on the `cursors` line has to
    explain, per spec.md: `min_by` returns the first of equal elements, so
    a full price+amount tie resolves to whichever venue sorts first under
    `Venue`'s `Ord`, i.e. Binance). Loop bounded by `while out.len() <
    TOP_N`, `filter_map` drops exhausted cursors. This comment is not
    optional — its omission is spec.md's specific example of how the
    determinism guarantee (test 4) silently degrades into "usually passes."
  - `merge()`: calls `merge_side()` for both sides, `best_bid`/`best_ask`
    via `?` on `.first()`, spread computed, `Summary` constructed — no
    explicit branch for empty map / one-sided book / single venue, all
    handled by early-return propagation, matching spec.md's "edge cases
    fall out of `?`" framing.
  - Signature (`pub fn merge(venues: &BTreeMap<Venue, &Book>) -> Option<Summary>`)
    is unchanged from step 4's `summarise()` — no caller anywhere else in
    the crate needs to change, which is the concrete reason `src/aggregator.rs`
    stays untouched this phase.
  - No `MergedBook` internal type — `merge()` keeps returning the proto
    type directly, per spec.md's explicit "leave it, document the tradeoff
    in the README instead" call (that documentation is Phase 3's job, not
    this phase's).
  - The eight tests, filed in this file's `#[cfg(test)] mod tests` per
    unit-test-by-access convention, each named as a behaviour sentence:
    `Side`-level (2): ask-prefers-lower/bid-prefers-higher; equal-price
    prefers larger amount on both sides. `merge`-level (6): two books
    produce correct top-ten and spread; equal price+amount across venues
    resolves deterministically (locks in the `BTreeMap` dependency); a
    crossed book produces a negative spread without panicking; a single
    venue works (no N=1 special-casing); no venues returns `None`; six
    levels returns six, not padded to ten.
  - **Test hazard, not applicable here but confirm it stays that way:**
    none of these eight tests drive the `watch` channel (all are pure,
    operating directly on `merge`/`merge_side`/`Side::better`), so the
    interleave-sends-with-reads hazard from `specs/005-aggregator/revisions.md`
    entry 3 does not apply — flagged in spec.md as a standing risk for this
    step given the number of multi-update scenarios, so confirm by
    inspection that no test in this file touches `tokio::sync::watch`.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — all eight tests pass, each identifiable individually by
    its behaviour-sentence name, not folded into an aggregate count.
    Specifically confirm test 4 (deterministic tie resolution) and test 5
    (crossed book → negative spread, no panic) — spec.md calls these the
    two that matter most.
  - `cargo run -- --pair ethbtc --port 50051`, then `grpcurl -plaintext
    127.0.0.1:50051 orderbook.OrderbookAggregator/BookSummary` (network
    permitting — report honestly if this environment can't reach both
    exchanges, per this project's standing honesty convention) — confirm
    levels with `exchange == "binance"` and `exchange == "bitstamp"` both
    appear in the same stream, proving real two-venue merging reached the
    wire for the first time in this project.
  - `git diff main --stat -- src/aggregator.rs` — zero diff (checkpoint 2
    of 3, the load-bearing one: this is the phase that does the actual
    algorithm work, so this is where a signature mismatch would first
    surface as an aggregator-side change).
  - `git diff main --stat` overall — confirm `src/merge.rs` is the only
    file touched.
- Done looks like: `merge()` genuinely combines both venues' sorted sides
  into a correctly ordered top-10 with the right tie-break and honest
  crossed-book behaviour, all eight tests pass, and no other file in the
  crate needed to change to make it compile.
- Commit boundary: `src/merge.rs` alone. Reverting this phase (with Phase 1
  still in place) restores single-venue `summarise()` on top of the already
  -cut README — a safe, buildable, if functionally step-4-equivalent, state.

### Phase 3: README merge pass + full verification gate

- Objective: Describe what Phase 2 actually shipped, once real merge
  behaviour exists to document — same "README describes what was shipped,
  not what was planned" rule this project has used since
  `specs/003-step-1-fixes/revisions.md` entry 1 — then run the complete
  verification gate once at the tip of the branch.
- Main changes: `README.md`.
  - Add to the design-decisions section grown in Phase 1: the merge
    algorithm described (peekable cursors over both venues' sorted sides,
    not sort-the-concatenation — framed honestly as "don't discard ordering
    the venues already did," per spec.md, not a speed claim), the tie-break
    rule stated as shipped fact now (larger amount wins a price tie, same
    rule both sides), crossed-book behaviour called out explicitly enough
    that a reader seeing a negative spread finds the explanation here
    rather than filing a bug.
  - Add to the production section: the internal-type decoupling tradeoff —
    `merge()` returns the `Summary`/`Level` proto types directly rather than
    through an internal `MergedBook`, and why that's fine at one consumer
    but would be worth revisiting with more.
  - Word count check again — additions here should keep the README close to
    Phase 1's target, not silently regrow it past ~150 lines / 1,200 words;
    if the merge description genuinely needs more room, prefer trimming
    elsewhere over letting the total creep back toward the pre-cut length.
- Verification (full gate, run once here at the tip of the branch):
  - `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D
    warnings`, `cargo fmt --check` — all clean.
  - `cargo test` reports all eight `merge.rs` tests individually, plus the
    full existing suite (the project's current baseline is 29 tests: 22
    unit, 6 + 1 integration — confirm the new total reflects +8 minus
    whatever `summarise`'s superseded assertions were, and report the
    actual count observed, not the arithmetic guess).
  - `grpcurl -plaintext 127.0.0.1:50051 orderbook.OrderbookAggregator/BookSummary`
    against a live `cargo run` — actual output quoted, confirming both
    venues' `exchange` labels appear and the spread is computed across
    them, not from one venue alone.
  - `docker compose up --build` — confirm it comes up through the
    configured proxy, both feeds connect, and the gRPC output through the
    container matches the direct-`cargo run` check above. Report actual
    observed behaviour, including any environment limitation, per this
    project's standing honesty convention (same as `005-aggregator`'s and
    `006-bitstamp`'s plans).
  - `git diff main --stat -- src/aggregator.rs` — zero diff (checkpoint 3
    of 3, final confirmation at the tip).
  - `git diff main --stat` overall — confirm the full branch touches only
    `README.md` and `src/merge.rs`, nothing else.
  - Read-through: no leftover references to single-venue output, no stale
    "step 5: not yet implemented" language anywhere in the README.
- Done looks like: every claim in the README matches what Phases 1-2
  actually shipped and what this phase actually verified in this
  environment; all three `git diff --stat -- src/aggregator.rs` checkpoints
  (Phases 1, 2, 3) show zero diff.
- Commit boundary: `README.md` alone (a second, distinct commit from
  Phase 1's — per spec.md, this pass lands "as part of/after the merge
  commit(s)," not folded backward into Phase 1's already-closed commit).
  Reverting it has no effect on build or test state; Phase 2's `merge()`
  keeps working with a README that's accurate through Phase 1 but silent on
  the merge itself.

## Cross-Cutting Considerations

- **`src/aggregator.rs` unchanged, checked at every phase, not just the
  end.** This is the step's structural contract, inherited from step 4's
  signature freeze. Checked explicitly in all three phases' verification
  sections above, not deferred to a single end-of-branch check — a
  regression introduced in Phase 2 that only gets caught in Phase 3 is
  still a regression that shipped a work-in-progress state in Phase 2's
  commit.
- **Commit message length.** Per direct instruction: short commit
  messages, no multi-paragraph bodies. Three commits, one line each is
  the right shape — e.g. "README: cut mechanics, grow design decisions",
  "merge: real two-venue merge with tie-break and crossed-book handling",
  "README: describe the merge, tie-break, and crossed-book behaviour".
- **`Side` as an enum, not a bool, is non-negotiable.** Called out in
  spec.md as a repeat of this project's standing silent-failure avoidance
  pattern (same reasoning as `Venue`, not a new rule invented for this
  step) — if implementation finds itself reaching for a bool parameter
  anywhere in the merge path, that's a stop-and-flag moment, not a
  simplification.
- **No `MergedBook` internal type this step.** Explicitly deferred to the
  README's production section as a documented tradeoff, not built — adding
  it would be scope creep against spec.md's explicit "leave it" call.
- **Dedup of identical publishes stays out of scope.** Confirmed by the
  user in spec.md's Open Questions — lands in step 6 with the rest of
  `src/aggregator.rs`'s work. No phase in this plan should touch dedup
  logic; if implementation finds itself wanting to compare a merged
  `Summary` against a "last published" value, that logic does not belong
  in `merge.rs` (which must stay pure — no notion of "last published") and
  does not belong in this branch at all.
- **Two-column watch-channel test hazard, checked, doesn't apply.** Phase
  2's tests are all pure, no `watch` channel involved — confirmed rather
  than assumed, per spec.md's explicit flag that this step has more
  multi-update scenarios than prior steps so the hazard is more likely to
  bite if a future test in this area strays from the pure-function pattern.
- **Untouched-files discipline**, same convention every prior step's plan
  in this repo has used: `proto/orderbook.proto`, `src/config.rs`,
  `src/telemetry.rs`, `src/proxy.rs`, `src/exchange/*`, `src/feed.rs`,
  `src/server.rs` should all show zero diff at the tip of this branch —
  this step's scope is `src/merge.rs` and `README.md` only. A phase whose
  diff unexpectedly touches any of these is a stop-and-flag condition.

## Verification Gates

Before this branch is considered ready to hand off:

- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all clean at the tip of the branch.
- All eight `src/merge.rs` tests pass, each identifiable individually by
  its behaviour-sentence name — tests 4 (deterministic tie resolution) and
  5 (crossed book → negative spread, no panic) specifically confirmed, per
  spec.md's own emphasis on these two.
- `grpcurl -plaintext 127.0.0.1:50051 orderbook.OrderbookAggregator/BookSummary`
  against a live `cargo run` shows levels from both `"binance"` and
  `"bitstamp"` in the same stream, and a spread computed across the two
  venues' best prices — actual output quoted, network permitting.
- `docker compose up --build` brings up both feeds and the gRPC server
  through the proxy — actual observed output reported, including any
  environment limitation.
- `git diff main --stat -- src/aggregator.rs` shows zero diff, checked at
  the tip of the branch (and was already checked identically at each of
  the three phases above).
- `git diff main --stat` at the tip shows only `README.md` and
  `src/merge.rs` touched across the whole branch.
- README cut-down landed as its own commit, before the merge-algorithm
  commit, and is under 1,200 words on its own (checked at Phase 1's tip,
  not just the final state).
- README's final state accurately describes the merge, the tie-break rule,
  crossed-book behaviour, and the internal-type tradeoff — matching what
  was actually shipped and actually verified in this environment.

## Expected Drift Triggers

If any of the following becomes true while implementing, update spec.md
before continuing rather than improvising past it:

- Phase 2 finds that `merge()`'s signature genuinely cannot stay
  `pub fn merge(venues: &BTreeMap<Venue, &Book>) -> Option<Summary>` — e.g.
  the `Peekable` cursor shape needs something `Aggregator` doesn't already
  provide in that borrowed-map form. This is the one condition most likely
  to force a `src/aggregator.rs` change, which would violate this branch's
  central scope check — flag immediately, don't quietly touch the
  aggregator to work around it.
- The required determinism comment on the `cursors` line turns out not to
  be sufficient to prevent a future reader from "fixing" `BTreeMap` to
  `HashMap` (e.g. because clippy or a linter flags the `BTreeMap` as
  unusual) — worth a stronger guard (a doc comment on the `Venue` enum
  itself, or a dedicated test asserting map type) rather than silently
  accepting the risk spec.md already flagged.
- The README, after Phase 1's cuts, is found to have removed information a
  reviewer would actually need (not just mechanics) — spec.md's Risks
  section anticipates this specifically; if it happens, restore the
  specific content rather than holding the word-count target as more
  important than substance.
- `docker compose up` cannot be run at all in this environment (no Docker
  daemon, no route to either exchange even through the configured proxy) —
  report this as "not verified here," not silently omitted, same standing
  rule every prior step's plan in this repo has used.
- Test count arithmetic in Phase 3 doesn't land where expected (e.g. more
  or fewer than 29 - (superseded `summarise` tests) + 8) — report the
  actual observed `cargo test` output rather than reconciling the number
  silently; a mismatch could mean a test was accidentally dropped or an
  unplanned one was added.
