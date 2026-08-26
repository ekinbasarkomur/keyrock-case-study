# Revisions: 012-kraken

Deviations and findings from spec.md/plan.md/tasks.md discovered during
implementation, recorded as they happen. This branch is research-only and
does not merge into `main`.

## 1. `RefCell` doesn't compile crate-wide — switched to `std::sync::Mutex`

**What the spec proposed**: "`pub struct Kraken { book: RefCell<Option<Book>> }` (or `Mutex`, though `RefCell` suffices since nothing here is genuinely concurrent...)".

**What actually happened**: `kraken.rs` built and its 15 unit tests passed
in isolation (via a temporary stub during Phase 2's implementation), but a
real crate-wide `cargo build` (Phase 5, wiring the third `feed::run_feed`
spawn into `main.rs`'s `JoinSet`) failed: `RefCell<Option<KrakenBook>>` is
`!Sync`, which makes any `Future` holding a `&Kraken` across an `.await`
point `!Send`, and `tokio::task::JoinSet::spawn` requires `Send` futures.
This is exactly the kind of gap an isolated module test can't catch —
`RefCell`'s single-threaded assumption is only violated by the *task
boundary*, not by any actual concurrent access.

**Fix**: `Kraken`'s held state is `std::sync::Mutex<Option<KrakenBook>>`
instead. Spec.md's own parenthetical already anticipated this as the
alternative; the "nothing here is genuinely concurrent" reasoning still
holds and is preserved in the updated doc comment — the `Mutex` exists to
satisfy the `Send` bound, not because two tasks ever actually touch it at
once. No test behavior changed; all 15 `kraken.rs` tests pass unedited
against the `Mutex` version.

**Lesson for future spec work**: "nothing here is genuinely concurrent" is
true about *data races* but not sufficient to justify `RefCell` the moment
the containing type crosses an `async move { ... }` task boundary — worth
checking against a real `tokio::spawn`/`JoinSet::spawn` call site, not just
reasoning about actual concurrent access, before picking `RefCell` over
`Mutex` in this codebase's architecture.

## 2. Phase 6 live verification — all real, all observed directly

- **Three-venue merge**: real `cargo run` (no Docker), `grpcurl` against
  `127.0.0.1:50157` showed a genuine combined `Summary` with `binance`,
  `bitstamp`, and `kraken` levels interleaved by price in the same
  response — e.g. bids alternating across all three exchanges at adjacent
  `0.0313xx` price levels.
- **Forced disconnect + re-subscribe**: adapted the project's existing
  Binance-blocking relay script (`relay.py`, used in `009-resilience`) into
  a Kraken-specific version — an HTTP CONNECT relay sitting in front of the
  real upstream proxy, refusing/killing only tunnels to a host containing
  `"kraken"` while passing Binance/Bitstamp through untouched. Blocking
  Kraken produced the expected sequence: `feed connection failed
  venue=kraken`, then a real jittered exponential backoff (`0.59s → 1.48s →
  3.62s → 4.85s → 14.03s → 35.25s`), confirming the same backoff machinery
  Binance/Bitstamp already use applies to Kraken with no special-casing
  needed.
- **Staleness exclusion**: ~30s after blocking (comfortably past the
  measured 12s threshold), `grpcurl` against the live stream showed zero
  `"kraken"` levels in the published `Summary` — only `binance`/`bitstamp`
  remained, confirming `Venue::Kraken`'s `staleness_threshold()` arm is
  actually consulted by `src/aggregator.rs`'s pre-merge filter, not just
  present in the `match`.
- **Reconnect discards prior state, doesn't resume it**: unblocking
  Kraken produced a fresh `connected` → `kraken status message` sequence
  (the same lifecycle a first connect produces), and `grpcurl` showed
  `kraken` levels reappearing in the merge within seconds — consistent
  with a fresh `snapshot` replacing the `Mutex`'s contents wholesale, per
  `a_fresh_snapshot_after_a_reconnect_replaces_prior_state_wholesale`'s
  unit-test evidence, now also confirmed live.
- **Checksum-mismatch recovery**: not independently re-verified live in
  this session beyond the unit test
  (`a_corrupted_checksum_clears_the_held_book`) — Kraken's checksum never
  actually mismatched during any of this session's live runs (real data,
  no corruption), so there was nothing to observe live. The unit test
  remains the evidence for this path; spec.md's flagged uncertainty about
  whether a mismatch-without-reconnect could leave Kraken silently stalled
  is answered by the unit test's assertion (a mismatch clears the held
  state and returns `None`, and the *next* real `update` — which normally
  wouldn't arrive without an intervening `snapshot` in Kraken's real
  protocol — also returns `None` rather than a wrong book), but a live
  reproduction of an actual on-wire checksum mismatch was not attempted.

## Summary

Both of spec.md's Open Questions requiring real capture/measurement
(price/qty type, staleness threshold) were resolved by live investigation
before the code that depended on them was written — not guessed. The one
architecture decision (interior-mutable state) required one real fix
(`RefCell` → `Mutex`) once exercised at the actual task-spawn boundary the
isolated implementation work couldn't see. All spec.md Acceptance Criteria
live-verification items were performed against a real Kraken connection,
not simulated.

This branch is research-only. `main`'s two-venue system is unaffected by
any of the above.
