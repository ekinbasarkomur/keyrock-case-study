# Plan: 009-resilience

## Summary

Six phases, one per commit in spec.md's own fixed "Commit order" section —
this is the one plan in this project's history where the phase count isn't a
planning judgment call at all, it's a direct copy of a sequencing
requirement spec.md already states explicitly. Padding or collapsing that
sequence would fight the spec, not simplify it.

- Phase 1 — `JoinSet` migration (`src/main.rs`), behaviour unchanged.
- Phase 2 — reconnection loop with backoff, jitter, and the stability-gated
  reset (`src/feed.rs`), pieces 2+3 of the spec combined into one commit per
  spec.md's own commit order.
- Phase 3 — per-venue token bucket (`src/feed.rs`, `src/exchange/mod.rs`).
- Phase 4 — staleness filter and per-venue thresholds
  (`src/aggregator.rs`, `src/exchange/mod.rs`).
- Phase 5 — grace period for never-seen data (`src/aggregator.rs`,
  `src/main.rs`'s exit policy).
- Phase 6 — client header venue status, including the client-side
  per-venue last-seen tracking (`src/bin/client.rs`), plus the README pass
  and the full verification gate run once at the tip.

This is the largest packet in the project's history (`complexity: large`,
touching five source files across seven design pieces), and it is also the
one with the most dangerous single failure mode: Phase 1 claims zero
behaviour change, and that claim is only as good as "existing tests pass
with zero assertion edits." Every later phase depends on Phase 1's
supervision shape being correct, so Phase 1 is treated below as a hard gate,
not just the first item in a list — nothing in Phase 2 starts until Phase
1's verification is clean.

Two structural facts are checked at **every** phase, not just the end, per
spec.md's Invariants section and this project's standing convention (see
`007-merge/plan.md`'s equivalent `src/aggregator.rs` checkpoint):

- **`git diff main --stat -- src/merge.rs` must show no diff.** Staleness is
  a pre-filter on the map handed to `merge()`; `merge()` itself never
  changes in this packet. This is the scope check's centerpiece.
- **The live proxy-interruption check is not deferred to a final phase.**
  This dev environment tunnels outbound connections through an HTTP CONNECT
  proxy configured by `PROXY_HOST`/`PROXY_PORT` in `.env` (`src/proxy.rs`,
  wired through `compose.yml`'s `HTTP_PROXY`/`HTTPS_PROXY` pass-through).
  Killing and restoring that proxy mid-run is this project's actual
  mechanism for simulating "Binance goes dark and comes back" without
  touching a real exchange's infrastructure. It becomes meaningfully
  testable starting Phase 2 (once there's a reconnect loop to observe) and
  is run again, more fully, in Phases 4-6 as staleness, grace period, and
  the client header each add something new for that live check to prove.

## Phase Breakdown

### Phase 1: `JoinSet` migration, behaviour unchanged — `src/main.rs` only

- Objective: Replace `select!` with a `JoinSet`-based supervisor that
  carries task identity (`Component::Feed(Venue) | Aggregator | Server`)
  alongside each result, while preserving today's exact exit policy — any
  task ending, for any reason, still exits the process. This phase's entire
  claim is "nothing observable changed," so it is graded on that claim
  holding, not on new capability.
- Main changes: `src/main.rs`.
  - `Component` enum and `TaskResult` type alias per spec.md's Piece 1.
  - Each `tokio::spawn` call wrapped in an async block normalising its
    result into `TaskResult` before the `JoinSet` sees it.
  - `join_next()` loop matching all four outcomes spec.md enumerates
    (`Ok(Ok(()))`, `Ok(Err(e))`, `Err(je) if je.is_panic()`, `Err(je)` for
    cancellation) — panic and cancellation logged distinguishably, not
    folded into one log line.
  - Exit policy: identical to today's `select!` — whichever task's result
    arrives first, the process exits, propagating the error if there was
    one. No feed-specific carve-out yet; that's Phase 5's job.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — **the load-bearing check for this whole phase**: report
    the exact diff in assertions, if any, between this run and the
    pre-Phase-1 baseline (32 tests: 25 unit, 6 + 1 integration, per the
    project map). Per spec.md, zero assertion edits are expected; call
    sites may move where a signature forces it, but if any existing
    assertion needed to change to make the suite pass, that is a stop-and
    -flag condition — it means the migration changed behaviour, not just
    plumbing, and Phase 1 must not proceed to Phase 2 until that's resolved
    or spec.md is updated to acknowledge it.
  - `cargo run -- --pair ethbtc --port 50051` against the live proxy (or
    direct connectivity, whichever this environment has) — confirm the
    process still starts, serves, and still exits promptly if a feed dies
    (kill the proxy briefly and confirm the process still exits today,
    since that's the behaviour this phase is required to preserve, not yet
    change).
  - `git diff main --stat -- src/merge.rs` — zero diff (checkpoint 1 of 6).
  - `git diff main --stat` overall — confirm only `src/main.rs` (plus
    `specs/009-resilience/`) touched.
- Done looks like: `JoinSet` fully replaces `select!`, identity travels with
  every result, and `cargo test`'s existing 32 assertions pass completely
  unedited — the concrete evidence for "behaviour unchanged," not just an
  assertion that it's true.
- Commit boundary: `src/main.rs` alone. Reverting it restores `select!` with
  no effect on any other file — every later phase depends on this one, so
  reverting Phase 1 alone means reverting the whole branch.

### Phase 2: reconnection, backoff, jitter, stability-gated reset — `src/feed.rs`

- Objective: `run_feed` never returns on a closed socket — it loops,
  backs off with jitter, and only resets that backoff once a connection has
  proven itself stable for 30s, not merely established. This is spec.md's
  "subtle piece" (Piece 3) and gets the most scrutiny of any single piece in
  this packet.
- Main changes: `src/feed.rs`.
  - Outer reconnect loop wrapping today's connect-and-read body.
  - Backoff sequence 1s/2s/4s/8s/16s, capped 30s; jitter multiplier drawn
    uniformly from `0.5x`-`1.5x` on every wait, via `rand` 0.10 (already a
    dependency, unused until now — its 0.10 API differs from the 0.8/0.9
    syntax most search results and training-data examples show, so work
    from the `docs.rs` page for the version actually resolved in
    `Cargo.lock`, not memory of an older API).
  - Reset gated on `connected_at.elapsed() > Duration::from_secs(30)`, not
    on `connect()` succeeding — the exact five-line fix spec.md specifies,
    landed as its own clearly identifiable piece of this commit.
  - Confirm during implementation (not assumed) that `subscribe_message`'s
    existing call site sits inside the new reconnect loop, so Bitstamp
    re-subscribes on every reconnect — per spec.md's explicit instruction
    to verify this rather than trust that the refactor preserved it.
  - Backoff state must accept an injected clock/`Instant` rather than
    calling `Instant::now()` internally, per the Testing Strategy's
    "pass `Instant` in as a parameter" requirement — this is what makes the
    two backoff tests below possible without `tokio::time::pause`.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — new unit tests filed in `src/feed.rs`'s `mod tests`,
    each individually identifiable:
    - `backoff grows and caps` — asserts the 1/2/4/8/16/30 sequence and
      that it does not grow past 30s.
    - `a short-lived connection does NOT reset the thrash loop` — the
      single most valuable test in this packet per spec.md: simulate a
      connect-then-immediately-drop cycle and assert the backoff keeps
      advancing rather than resetting to 1s every time.
    - `a stable connection does reset` — simulate a connection held past
      the 30s stability window and assert the next backoff starts back at
      1s.
    - `jitter stays within range` — assert every produced wait falls
      inside `[0.5x, 1.5x]` of its nominal value, across enough samples to
      catch "jitter not applied at all" as well as "applied out of range."
  - Existing 32-test baseline still passes unedited (same standard as
    Phase 1, now extended by the new tests above rather than replaced).
  - **Live proxy-interruption check, run here for the first time in this
    packet, not deferred:** with `cargo run -- --pair ethbtc --port 50051`
    live against the configured proxy, kill the proxy process (or block
    `PROXY_HOST:PROXY_PORT`), and with `RUST_LOG=debug` observe the Binance
    feed logging reconnect attempts with growing, jittered delays rather
    than the process exiting. Restore the proxy and observe a successful
    reconnect. Report the actual observed log lines and timings, not just
    "it reconnected" — this is the truth anchor the pure unit tests above
    cannot provide by themselves, per spec.md's Testing Strategy section.
  - `git diff main --stat -- src/merge.rs` — zero diff (checkpoint 2 of 6).
  - `git diff main --stat` overall — confirm only `src/feed.rs` touched
    since Phase 1.
- Done looks like: a feed task that survives a dropped connection
  indefinitely, backs off correctly, jitters correctly, and — the phase's
  namesake subtlety — does not thrash into Binance's rate limit on a
  connect-then-immediately-drop pattern. Confirmed both by the four unit
  tests and by watching a real reconnect happen against the live proxy.
- Commit boundary: `src/feed.rs` alone. Reverting it (with Phase 1 still in
  place) returns to a feed that ends on any disconnect, but under the new
  `JoinSet` supervisor from Phase 1 rather than the old `select!` — still a
  safe, buildable state, just without resilience yet.

### Phase 3: token bucket — `src/feed.rs`, `src/exchange/mod.rs`

- Objective: An absolute per-venue connection-rate ceiling, composed with
  (not replacing) Phase 2's backoff — backoff answers "when do I try next,"
  the bucket answers "am I allowed to try."
- Main changes:
  - `src/exchange/mod.rs`: `Venue::connect_rate(self) -> (f64, f64)`
    (capacity, tokens/sec), living next to the eventual
    `staleness_threshold` (Phase 4) so every per-venue fact is in one place
    and both `match`es stay exhaustive. Binance: refill 1 token/sec (from
    its documented 300-per-5-minutes), capacity 5 (deliberately small — a
    capacity of 300 would let a single burst spend the whole five-minute
    allowance at once). Bitstamp: a conservative guess, documented in code
    as undocumented — Bitstamp publishes no connection-rate limit, and the
    comment must say so plainly rather than presenting an invented number
    as fact.
  - `src/feed.rs`: `bucket.acquire().await` inserted between
    `backoff.wait().await` and `connect().await`, per spec.md's exact
    composition. Code comment explaining the bucket's actual purpose (a
    documented absolute ceiling expressed directly, not a claim that
    today's backoff is insufficient in practice — spec.md is explicit
    that under normal reconnect behaviour the bucket is essentially never
    reached, roughly 14 attempts in 5 minutes against Binance's 300 limit).
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — new tests:
    - `bucket empties and refills` — drain the bucket, assert tokens
      return over simulated time.
    - `bucket doesn't exceed capacity` — assert a burst of attempts is
      throttled rather than all slipping through unthrottled.
  - Existing baseline (Phase 1 + Phase 2's tests) still passes unedited.
  - Live check: repeat Phase 2's proxy-kill/restore observation and confirm
    reconnect behaviour is unchanged in the common case (the bucket should
    be invisible under normal backoff cadence, per spec.md's own framing)
    — if reconnects visibly slow down or stall under the bucket during a
    normal single-outage test, that's a sign the capacity/refill numbers
    are wrong, not that the live check itself is unnecessary.
  - `git diff main --stat -- src/merge.rs` — zero diff (checkpoint 3 of 6).
  - `git diff main --stat` overall — confirm only `src/feed.rs` and
    `src/exchange/mod.rs` touched since Phase 2.
- Done looks like: a documented, per-venue absolute rate ceiling that
  composes with backoff without changing observed reconnect behaviour in
  the common single-outage case, plus both bucket tests passing.
- Commit boundary: `src/feed.rs`, `src/exchange/mod.rs`. Reverting it (with
  Phases 1-2 in place) returns to backoff-only reconnection with no
  absolute ceiling — a real gap against Binance's documented limit, but
  still a buildable, functioning state.

### Phase 4: staleness filter and per-venue thresholds — `src/aggregator.rs`, `src/exchange/mod.rs`

- Objective: Exclude a venue's book from the merge input once its
  last-update time exceeds a per-venue threshold, so a reconnecting venue's
  stale prices stop poisoning the combined top-of-book — without touching
  `merge()` itself.
- Main changes:
  - `src/exchange/mod.rs`: `Venue::staleness_threshold(self) -> Duration`
    — Binance tight (silence itself means failure, since it publishes a full
    snapshot every ~100ms whether or not the book changed; a threshold in
    the 1-2s range gives a few missed heartbeats' grace before treating
    that as a dead connection, without the multi-second window Bitstamp
    needs for its on-change-only cadence), Bitstamp **8 seconds**, per the
    spec's already-resolved
    live measurement (792 messages over ~5.25 minutes, max observed gap
    1.795s, 4x that rounded up) — this plan treats 8s as a settled input,
    not something to re-measure.
  - `src/aggregator.rs`: before calling `merge::merge(&venues)`, filter the
    `BTreeMap<Venue, &Book>` by
    `now.duration_since(s.last_update) < v.staleness_threshold()`, with
    `now = Instant::now()` hoisted once per pass (not called per-venue
    inside the filter closure) so every venue in the same tick is judged
    against the identical instant — per spec.md's explicit reasoning for
    why this matters.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — new unit tests, each taking an injected `Instant` rather
    than calling `Instant::now()` internally:
    - `stale venue excluded from merge` — a venue past its threshold is
      absent from what's handed to `merge()`.
    - `fresh venue included` — a venue just under its threshold survives
      the filter (catches a threshold set too tight).
    - `thresholds differ per venue` — Binance and Bitstamp's thresholds
      are asserted distinct, catching the two collapsing to one shared
      value.
  - **`git diff main --stat -- src/merge.rs` — zero diff, checkpoint 4 of
    6 and the single most important checkpoint in this whole plan**: this
    is the phase where the temptation to fold "staleness" logic into
    `merge()` (since staleness is itself time-based) is highest, per
    spec.md's own Scope section. Any nonzero diff here is a stop-and-flag
    condition, not a design call to make unilaterally.
  - **Live proxy-interruption check, the packet's core acceptance
    criterion, run here for real for the first time:** with the full
    binary running against the live proxy and a real client attached (or
    `grpcurl` streaming, if Phase 6's client isn't landed yet — note which
    was used), kill the proxy, wait past Bitstamp's... no — past
    **Binance's** threshold (the tight one, since the proxy carries
    Binance's connection in this project's setup) and confirm the streamed
    `Summary` narrows to Bitstamp-only levels rather than continuing to
    publish stale Binance prices. Restore the proxy and confirm Binance
    levels return once reconnected (Phase 2) and fresh again (this phase).
    Report actual observed gRPC output (via `grpcurl` at minimum), not just
    "it worked."
  - `git diff main --stat` overall — confirm only `src/aggregator.rs` and
    `src/exchange/mod.rs` touched since Phase 3.
- Done looks like: a demonstrably stale venue's book never reaches
  `merge()`, the three new unit tests pass, `src/merge.rs` shows zero diff,
  and the live proxy-kill test shows the combined book actually narrowing
  to the surviving venue in real gRPC output.
- Commit boundary: `src/aggregator.rs`, `src/exchange/mod.rs`. Reverting it
  (Phases 1-3 in place) returns to a reconnecting-but-never-filtered
  aggregator — reconnection works, but a mid-reconnect venue's stale book
  still pollutes the merge until data flows again.

### Phase 5: grace period for never-seen data — `src/aggregator.rs`, `src/main.rs`

- Objective: Distinguish "a venue went quiet after producing data" (Phase
  4's job) from "no venue has ever produced data for this pair" (this
  phase's job) — the latter needs to be fatal, since Bitstamp accepts any
  channel name without validation and would otherwise run forever producing
  nothing observable.
- Main changes:
  - `src/aggregator.rs`: track `started_at: Instant` for the aggregator
    task; if `started_at.elapsed() > GRACE && self.venues.is_empty()`, log
    an error naming the pair and return — making the aggregator's own
    return fatal again, distinctly from a feed's return (which, since
    Phase 2, is no longer fatal). `GRACE = Duration::from_secs(60)`, per the
    spec's already-resolved reasoning (covers one full backoff cycle,
    `1+2+4+8+16+30 = 61s`) — this plan treats 60s as a settled input, not
    something to re-derive.
  - `src/main.rs`: confirm the `Component::Aggregator` and
    `Component::Server` arms of the Phase 1 `JoinSet` loop still exit the
    process on return/error (unchanged from Phase 1), while
    `Component::Feed(_)` returning is no longer treated as fatal — a feed
    task's own loop no longer returns except on panic, per Phase 2, so this
    is really confirming the supervisor's policy statement is accurate
    post-Phase-2/4/5, not adding new branching logic.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — new unit test: `empty map past grace exits` — with an
    injected clock past `GRACE` and an empty venue map, assert the
    aggregator signals exit (piece 6's core guarantee per spec.md's Testing
    Strategy).
  - Existing baseline (Phases 1-4's tests) still passes unedited.
  - Real-binary check: `cargo run -- --pair xyzabc --port 50051` (or
    equivalent bad-pair invocation) and confirm the process exits within
    ~60s with a log message naming the pair — this is one of spec.md's
    explicit Acceptance Criteria, run here directly rather than only
    inferred from the unit test.
  - Live proxy check, extended: with a **valid** pair and the proxy live,
    kill the proxy for longer than the grace period and confirm the process
    does **not** exit (Bitstamp is still producing data, so `venues` is
    non-empty and the grace-period branch never fires) — this distinguishes
    "one venue reconnecting" from "no venue ever produced data" in a real
    run, which is the exact distinction this phase exists to draw.
  - `git diff main --stat -- src/merge.rs` — zero diff (checkpoint 5 of 6).
  - `git diff main --stat` overall — confirm only `src/aggregator.rs` and
    `src/main.rs` touched since Phase 4.
- Done looks like: a genuinely bad pair name exits within the grace period
  with a clear log message, while a valid pair with one venue mid-reconnect
  keeps running — both confirmed by real runs, not just the unit test.
- Commit boundary: `src/aggregator.rs`, `src/main.rs`. Reverting it (Phases
  1-4 in place) returns to a process that would spin forever on a bad pair
  name, producing nothing — a real regression against spec.md's Acceptance
  Criteria, but still a buildable state for the purpose of isolating a
  revert.

### Phase 6: client header venue status + README + full verification gate — `src/bin/client.rs`, `README.md`, `compose.yml`

- Objective: Surface per-venue health in the client's terminal header —
  the human-visible verification surface for Phases 2-5 — and close out the
  packet with the README updates spec.md's Acceptance Criteria require,
  plus one full verification gate run at the tip of the branch.
- Main changes:
  - `src/bin/client.rs`:
    - A client-side `HashMap<Venue, Instant>` (or equivalent) tracking
      last-seen-in-a-frame per venue, updated whenever a streamed `Summary`
      contains at least one level from that venue. This is the piece added
      as a spec correction, not cosmetic — without it there is no duration
      to display, only presence/absence.
    - Header render, using the venue-list-taking function shape already set
      up in step 6/`008-client` for this purpose (per spec.md, "filling in
      a field rather than restructuring the header"): `● ` for a venue seen
      in the most recent frame, `○ stale <Ns>` computed from the client's
      own last-seen map, not any server-side signal.
  - `README.md`:
    - Reconnection section: jittered backoff and — explicitly, not just the
      mechanism — *why* the reset waits for stability (Phase 2/Piece 3's
      reasoning about the thrash pattern).
    - Staleness section: per-venue thresholds, and where Bitstamp's 8s
      number came from (the actual measured numbers — 792 messages, 1.795s
      max gap — not just the chosen threshold, per spec.md's explicit
      request).
    - Grace period section: 60s and the backoff-cycle reasoning
      (`1+2+4+8+16+30=61s`).
    - Connection rate limits section: Binance's documented 300/5min ceiling
      and Bitstamp's explicitly-labelled undocumented conservative guess.
    - Production section: a note that venue health belongs on the wire in a
      real system, naming the concrete blind spot spec.md specifies (a
      venue publishing normally but never making the top 10 looks
      identical to a genuinely stale venue) as the reason, not a general
      principle.
    - Fix the two places (Docker section, top-of-file summary — spec.md is
      explicit both need checking, not just one) that currently claim the
      container exits shortly after startup if it can't reach Binance —
      that described the old `select!` exit-on-failure behaviour this
      packet removes.
    - Word budget: README was 1,157 words before this step; this step adds
      the most of any step so far — keep under ~1,400 words per spec.md,
      trimming elsewhere if it would exceed that.
  - `compose.yml`: same check as the README — confirm no comment or
    documentation line in this file still describes the old exit-on
    -failure behaviour (spec.md names this file specifically alongside
    the README).
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — no new tests expected from the client itself (matching
    `008-client`'s standing "no tests for `client.rs`" convention — this is
    a rendering/manual-verification surface, not a logic surface); report
    the actual final test count observed, building on whatever Phases 1-5
    added.
  - `wc -w README.md` — confirm the final word count and that it's under
    the ~1,400-word ceiling; report the actual number.
  - **The packet's full acceptance-criteria live check, run completely
    here, once, end to end:**
    - `cargo run -- --pair ethbtc --port 50051` and
      `cargo run --bin client -- --addr http://127.0.0.1:50051` both live,
      through the configured proxy.
    - Kill the proxy mid-run: confirm the client's header shows Binance
      transitioning from `●` to `○ stale <Ns>` with the displayed duration
      growing, the streamed book visibly narrowing to Bitstamp-only levels,
      and the server process itself staying up (no exit, no restart)
      throughout.
    - Restore the proxy: confirm Binance's indicator returns to `●` and
      Binance levels reappear in the combined book, without any process
      restart on either binary.
    - Report the actual observed header lines and timings — this is the
      packet's real acceptance evidence, not a description of what should
      happen.
  - `docker compose up --build` — confirm it survives the same
    proxy-kill/restore cycle without the container exiting, per spec.md's
    explicit Acceptance Criteria line. Report actual observed behaviour,
    including any environment limitation, per this project's standing
    honesty convention.
  - `--pair xyzabc` re-run once more here, end to end through the real
    binary, confirming the grace-period exit still holds after all six
    phases are combined.
  - `git diff main --stat -- src/merge.rs` — zero diff (checkpoint 6 of 6,
    final confirmation at the tip).
  - `git diff main --stat` overall — confirm the whole branch touches only
    `src/main.rs`, `src/feed.rs`, `src/exchange/mod.rs`,
    `src/aggregator.rs`, `src/bin/client.rs`, `README.md`, `compose.yml`,
    and `specs/009-resilience/` — no other path.
- Done looks like: every README claim matches what Phases 1-5 actually
  shipped and what this phase actually observed running it; the client
  visibly narrates a real proxy kill/restore cycle without the reviewer
  needing to read logs; all six `src/merge.rs` checkpoints across the whole
  branch show zero diff; and the full verification gate is clean at the
  tip.
- Commit boundary: `src/bin/client.rs` as one commit (the venue-status
  feature), `README.md` + `compose.yml` as a second, smaller commit (the
  doc pass) — matching this project's standing pattern (`007-merge`,
  `008-client`) of landing a doc pass as its own commit, separable from the
  feature it describes. Reverting the doc commit alone has no effect on
  runtime behaviour; reverting the client commit alone returns to a
  functioning server with no visible venue-status surface, but Phases 1-5's
  actual resilience behaviour is unaffected either way.

## Cross-Cutting Considerations

- **`src/merge.rs` zero-diff, checked at all six phases, not just the
  end.** This is the packet's structural contract, inherited from every
  prior step since `merge()` was first made pure. A regression introduced
  in Phase 4 (the phase under the most pressure to touch it, per spec.md)
  that only gets caught in Phase 6 is still a regression that shipped a
  work-in-progress state in Phase 4's own commit.
- **The live proxy-interruption test is not a one-time final check.** It
  appears in Phase 2 (first reconnect observed), Phase 3 (confirms the
  bucket is invisible in the common case), Phase 4 (confirms the combined
  book actually narrows — the packet's core acceptance criterion), Phase 5
  (confirms grace period does not fire on a mid-reconnect valid pair), and
  Phase 6 (the full kill/restore cycle with the client header visibly
  narrating it). Each phase's live check is scoped to what that phase
  actually added — Phase 6 is where they compose into the packet's stated
  Acceptance Criteria in full.
- **Phase 1 is a hard gate, not just first in sequence.** Every later phase
  depends on `JoinSet`'s exit-policy plumbing being correct and genuinely
  behaviour-unchanged. If Phase 1's `cargo test` run needs even one
  assertion edited, stop and resolve that before starting Phase 2 — do not
  proceed on the assumption it'll wash out later.
- **Piece 3 (stability-gated reset) is this packet's single highest-risk
  design point**, per spec.md's own "the subtle piece" framing. The `a
  short-lived connection does NOT reset the thrash loop` test in Phase 2 is
  the one test in this entire packet spec.md singles out as the most
  valuable — treat a failure there as the top priority to resolve correctly
  before moving to Phase 3, not a test to loosen.
- **Both previously-open questions (Bitstamp's 8s threshold, the 60s grace
  period) are settled inputs for this plan, not re-derivable.** Phase 4 and
  Phase 5 implement the measured/confirmed numbers from spec.md's Open
  Questions section directly; neither phase re-measures or re-justifies
  them from scratch, only records the existing reasoning in the README
  (Phase 6).
- **Per-venue facts live in one place.** `Venue::connect_rate` (Phase 3) and
  `Venue::staleness_threshold` (Phase 4) both belong on `Venue` in
  `src/exchange/mod.rs`, matching spec.md's explicit instruction — a
  per-venue `match` anywhere else in the codebase for either fact is a
  design smell to flag, not a convenience to take.
- **What's dropped stays dropped.** No give-up-after-N-failures counter, no
  `live_feeds` counter, no `bts:error` special path beyond updating its
  existing comment, no panic-recovery — spec.md's "What's dropped, and why"
  section is explicit these were considered and rejected, not overlooked.
  If implementation finds itself reaching for any of these mid-phase,
  that's a scope deviation from spec.md, not a natural extension.
- **Commit message length.** Per this project's standing convention: short,
  one-line commit messages. Seven commits total (Phase 6 splits into two) —
  e.g. "JoinSet migration, behaviour unchanged", "reconnect: backoff,
  jitter, stability-gated reset", "feed: per-venue token bucket",
  "aggregator: staleness filter", "aggregator: grace period for
  never-seen data", "client: venue status header", "README: reconnection,
  staleness, grace period, rate limits".
- **Untouched-files discipline.** `src/model.rs`, `src/proxy.rs`,
  `src/server.rs`, `proto/orderbook.proto`, `Dockerfile`,
  `rust-toolchain.toml` should all show zero diff at the tip of this
  branch — this packet's scope is the seven files named in spec.md's Scope
  section (plus `compose.yml`'s comment fix). A phase whose diff
  unexpectedly touches any of these is a stop-and-flag condition.

## Verification Gates

Before this branch is considered ready to hand off:

- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all clean at the tip of the branch.
- `cargo test` reports the existing baseline (32 tests as of `main`) plus
  every new test named in spec.md's Testing Strategy, each individually
  identifiable by its behaviour-sentence name — report the actual observed
  count, not the arithmetic guess.
- Phase 1's `cargo test` run at the time it landed showed **zero** edited
  assertions against the pre-Phase-1 baseline — confirmed, not assumed.
- The single highest-priority test in this packet, `a short-lived
  connection does NOT reset the thrash loop`, passes and is confirmed to
  actually exercise the connect-then-immediately-drop pattern, not a
  simplified stand-in for it.
- Live proxy kill/restore, run in full at Phase 6's tip: Binance goes
  stale in the client header, the combined book narrows to Bitstamp-only,
  the server process stays up throughout, and Binance recovers on proxy
  restore without any process restart — actual observed output reported.
- `--pair xyzabc` (or equivalent) exits within the grace period with a log
  message naming the pair — run at the real binary, not only inferred from
  the unit test.
- `docker compose up` survives the same proxy interruption without the
  container exiting.
- `git diff main --stat -- src/merge.rs` shows zero diff — checked
  identically at all six phase checkpoints above, not just here.
- `git diff main --stat` at the tip shows only the files named in spec.md's
  Scope section plus `compose.yml`'s comment fix.
- README's final state accurately describes reconnection/backoff/jitter and
  *why* the reset waits for stability, the measured Bitstamp staleness
  threshold with its actual numbers, the grace period and its reasoning,
  and both venues' connection-rate limits (Bitstamp's explicitly labelled
  as an undocumented guess) — matching what was actually shipped and
  actually observed in this environment.

## Expected Drift Triggers

If any of the following becomes true while implementing, update spec.md
before continuing rather than improvising past it:

- Phase 1's `cargo test` run requires editing an existing assertion to
  pass — per spec.md, this means the migration did something it should not
  have; stop, diagnose whether it's a genuine behaviour change hiding in
  the refactor, and either fix the refactor or get the behaviour change
  explicitly acknowledged in spec.md before proceeding to Phase 2.
- The `a short-lived connection does NOT reset the thrash loop` test (Phase
  2) is hard to express as a pure unit test with an injected clock and
  starts to need real `tokio::time::pause` or wall-clock sleeps — worth
  flagging, since spec.md's Testing Strategy explicitly commits to the
  injected-clock approach specifically to avoid that dependency.
- Bitstamp's actual observed connection-rate behaviour during the live
  proxy checks (Phases 2-6) suggests the chosen conservative
  capacity/refill numbers are meaningfully wrong in either direction (too
  throttled, or clearly insufficient) — spec.md already frames this number
  as a guess subject to revision, so a concrete observation here is exactly
  the kind of evidence that should feed back into the README's stated
  number, not be silently absorbed.
- The live proxy-kill test reveals that Binance's tight staleness threshold
  (Phase 4) triggers false "stale" flapping during normal operation (not
  during an actual proxy kill) — this would mean the 1-2s range this plan
  reasons to from Binance's 100ms publish cadence needs its own live
  measurement pass, the same way Bitstamp's 8s threshold was measured,
  rather than being accepted on that reasoning alone.
- `docker compose up` cannot be run at all in this environment (no Docker
  daemon, no route to either exchange even through the configured proxy) —
  report this as "not verified here," not silently omitted, same standing
  rule every prior step's plan in this repo has used.
- The client's per-venue last-seen inference (Phase 6) is found to
  misfire in a way beyond the one blind spot spec.md already names (a
  venue publishing but never making the top 10) — e.g. a false "stale"
  reading during genuinely normal two-venue operation — worth flagging as
  a second blind spot for the README rather than silently accepted as
  expected behaviour.
- Any phase's `git diff main --stat` touches a file outside its declared
  scope (especially `src/merge.rs`, `src/server.rs`, or `src/model.rs`) —
  stop and reconcile before continuing, rather than folding an unplanned
  change into the same commit.
