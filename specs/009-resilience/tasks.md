# Tasks: 009-resilience

## Task Writing Rules

- Each task should describe a real unit of progress.
- Each task should name the expected files or areas touched.
- Each task should include explicit verification.
- Prefer behavior-level verification over mock-only checks.

Six phases below map 1:1 to spec.md's fixed commit order — do not reorder or
merge phases, and do not fold a later phase's work into an earlier commit
even if it would be convenient to do so while a file is already open.

Two checks repeat at every phase, not just at the end, because the packet's
own risk profile puts the highest cost on catching a regression late:

- `git diff main --stat -- src/merge.rs` must show no diff, checked once per
  phase. Staleness (Phase 4) is the phase under the most pressure to fold
  logic into `merge()`, since staleness is itself time-based — that is
  exactly why the check is repeated at every phase rather than trusted to
  hold once confirmed in Phase 4 alone.
- The live proxy-interruption check (kill/restore `PROXY_HOST`/`PROXY_PORT`
  mid-run) is run in Phases 2 through 6, each time scoped to what that phase
  actually added, not deferred to a single final task.

None of the four explicitly-rejected mechanisms — a give-up-after-N-failures
counter, a `live_feeds` counter, a `bts:error` special path beyond its
existing comment, or panic-recovery — appear in any task below. If
implementation drifts toward any of them mid-task, stop and flag it rather
than treating it as a natural extension.

## Phase 1: `JoinSet` migration, behaviour unchanged — `src/main.rs`

### 1. Replace `select!` with a `JoinSet`-based supervisor

- Files or areas: `src/main.rs` only.
- Change:
  - Add `#[derive(Debug, Clone, Copy)] enum Component { Feed(Venue),
    Aggregator, Server }` and `type TaskResult = (Component,
    Result<(), anyhow::Error>);`.
  - Wrap each of today's four `tokio::spawn` calls (Binance feed, Bitstamp
    feed, aggregator, server) in an async block that normalises its result
    into `TaskResult` before handing it to a `tokio::task::JoinSet`.
  - Replace the `select!` block with a `tasks.join_next().await` loop
    matching all four outcomes: `Some(Ok((c, Ok(()))))`,
    `Some(Ok((c, Err(e))))`, `Some(Err(je)) if je.is_panic()`,
    `Some(Err(je))` (cancellation), `None`. Panic and cancellation must be
    logged distinguishably — not folded into one log line.
  - Exit policy for this task only: identical to today's `select!` — the
    first task result received (of any kind, any `Component`) ends the
    process, propagating the error if there was one. No feed-specific
    carve-out yet; that is Phase 5's job, not this one's.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build` — clean.
  - `cargo test` — **the load-bearing check for this task.** Compare against
    the pre-branch baseline (32 tests: 25 unit, 6 + 1 integration). Report
    the exact diff in assertions observed, if any. Per spec.md, zero
    assertion edits are expected — call sites may move where a signature
    forces it, but if a single existing assertion needed editing to make the
    suite pass, that is a stop-and-flag condition, not something to patch
    around and continue past. Do not start Phase 2 until this is either
    resolved or spec.md is explicitly updated to acknowledge a behaviour
    change.
  - Manual: `cargo run -- --pair ethbtc --port 50051` against the configured
    proxy (or direct connectivity, whichever this environment has); confirm
    it starts and serves. Then kill the proxy briefly and confirm the
    process still exits promptly — that is today's required behaviour,
    unchanged by this task; Phase 2 is what removes it.
  - `git diff main --stat -- src/merge.rs` — zero diff (checkpoint 1 of 6).
  - `git diff main --stat` overall — confirm only `src/main.rs` (plus
    `specs/009-resilience/`) is touched.
- Done when:
  - `JoinSet` fully replaces `select!`, task identity travels with every
    result via `Component`, and `cargo test`'s existing 32 assertions pass
    completely unedited — the concrete evidence for "behaviour unchanged,"
    not an assertion that it's true.
  - This lands as its own commit (e.g. `JoinSet migration, behaviour
    unchanged`).

## Phase 2: reconnection, backoff, jitter, stability-gated reset — `src/feed.rs`

### 2. Add the outer reconnect loop with backoff and jitter

- Files or areas: `src/feed.rs` only.
- Change:
  - Wrap today's connect-and-read body in an outer loop so `run_feed` never
    returns on a closed socket (except on panic — that stays fatal per the
    Phase 1 supervisor policy, unchanged here).
  - Backoff sequence 1s, 2s, 4s, 8s, 16s, capped at 30s. Backoff state must
    accept an injected clock/`Instant` rather than calling `Instant::now()`
    internally, so the tests in task 3 can supply a fixed clock without
    `tokio::time::pause`.
  - Jitter every wait by a multiplier drawn uniformly from `0.5x`-`1.5x`,
    via `rand` 0.10 — read the `docs.rs` page for the resolved 0.10 API
    rather than 0.8/0.9-era examples, which use a different surface.
  - Confirm, by reading the resulting code (not by assuming the refactor
    preserved it), that the existing `subscribe_message` call site now sits
    inside the new reconnect loop, so Bitstamp re-subscribes on every
    reconnect (its subscription is per-connection). Report explicitly which
    was true before this task's edit and after.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build` — clean.
  - Covered together with task 3's tests — see below; no standalone
    `cargo test` run needed for this task alone since the two land in the
    same file/commit.
- Done when:
  - `run_feed` loops on a closed socket rather than returning, waits follow
    the capped sequence with jitter applied, and Bitstamp's
    `subscribe_message` call is confirmed to be inside the reconnect loop.

### 3. Backoff/jitter/stability-reset unit tests

- Files or areas: `src/feed.rs`, `#[cfg(test)] mod tests` block only.
- Change: add four tests, each named as a behaviour sentence, each stating
  the bug it catches:
  - `backoff grows and caps` — asserts the 1/2/4/8/16/30 sequence and that
    it never exceeds 30s.
  - `a short-lived connection does NOT reset the thrash loop` — **the single
    most valuable test in this whole packet.** Simulate a
    connect-then-immediately-drop cycle (well under the 30s stability
    window) and assert the backoff keeps advancing on each cycle rather
    than resetting to 1s — this is the only thing standing between the
    shipped code and actually hitting Binance's 300-per-5-minute limit, per
    spec.md's own framing.
  - `a stable connection does reset` — simulate a connection held past the
    30s stability window and assert the next backoff starts back at 1s;
    catches a stability window that effectively never fires.
  - `jitter stays within range` — across enough samples to catch both "out
    of the 0.5x-1.5x band" and "not applied at all," assert every produced
    wait falls inside that band of its nominal value.
  - Add the stability-gated reset itself as part of this task if not
    already covered by task 2 — `if connected_at.elapsed() >
    Duration::from_secs(30) { backoff.reset(); }`, gated on elapsed
    connection time, not on `connect()` succeeding.
- Verification:
  - `cargo test feed::` — all four tests run and pass, individually
    identifiable by name.
  - `cargo test feed::a_short_lived_connection_does_not_reset_the_thrash_loop`
    (or its actual generated identifier) run individually and confirmed to
    genuinely exercise the connect-then-immediately-drop pattern, not a
    simplified stand-in for it — quote the test body's shape in the report.
  - Full `cargo test` — existing 32-test baseline plus these four, all
    unedited; report the actual total observed.
- Done when:
  - All four tests exist with the names above, pass individually, and the
    thrash-loop test is confirmed (by inspection, quoted in the report) to
    actually simulate a connect-then-drop cycle rather than a shortcut.

### 4. Phase 2 live proxy check and verification gate

- Files or areas: none (verification-only task).
- Change: none.
- Verification:
  - **Live proxy-interruption check, run here for the first time in this
    packet.** With `cargo run -- --pair ethbtc --port 50051` live against
    the configured proxy and `RUST_LOG=debug`, kill the proxy (stop the
    process, or otherwise block `PROXY_HOST:PROXY_PORT`). Observe the
    Binance feed logging reconnect attempts with growing, jittered delays
    rather than the process exiting. Restore the proxy and observe a
    successful reconnect. Quote the actual observed log lines and timings
    — "it reconnected" alone is not sufficient evidence.
  - `git diff main --stat -- src/merge.rs` — zero diff (checkpoint 2 of 6).
  - `git diff main --stat` overall — confirm only `src/feed.rs` has changed
    since Phase 1.
- Done when:
  - A feed task is observed, live, surviving a dropped connection
    indefinitely with correctly growing and jittered backoff, and the two
    checkpoint diffs both confirm scope. This lands as its own commit (e.g.
    `reconnect: backoff, jitter, stability-gated reset`).

## Phase 3: per-venue token bucket — `src/feed.rs`, `src/exchange/mod.rs`

### 5. Add `Venue::connect_rate` and wire the bucket into the reconnect loop

- Files or areas: `src/exchange/mod.rs`, `src/feed.rs`.
- Change:
  - `src/exchange/mod.rs`: `impl Venue { fn connect_rate(self) -> (f64,
    f64); }` (capacity, tokens/sec) as an exhaustive `match` alongside where
    `staleness_threshold` (Phase 4) will also live — both per-venue facts
    belong on `Venue`, not scattered as a separate `match` elsewhere.
    Binance: capacity 5, refill 1 token/sec (from its documented
    300-per-5-minutes limit; capacity deliberately small — 300 would let a
    single burst spend the entire five-minute allowance at once). Bitstamp:
    a conservative guess, documented in a code comment as explicitly
    undocumented — Bitstamp publishes no connection-rate limit, and the
    comment must say so plainly rather than presenting an invented number as
    fact.
  - `src/feed.rs`: insert `bucket.acquire().await` between
    `backoff.wait().await` and `connect().await`, per spec.md's exact
    composition (backoff answers "when," the bucket answers "am I
    allowed"). Add a code comment stating the bucket's actual purpose: under
    normal backoff behaviour a reconnecting venue produces roughly 14
    attempts in 5 minutes against Binance's 300 limit, so the bucket is
    essentially never reached in the common case — it exists to express a
    *documented* ceiling directly, not because backoff alone is known to be
    insufficient.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build` — clean.
  - Covered together with task 6's tests below.
- Done when:
  - `Venue::connect_rate` exists with the stated numbers and rationale in
    comments, and `bucket.acquire().await` sits between backoff and connect
    in `run_feed`'s loop.

### 6. Token bucket unit tests

- Files or areas: `src/feed.rs`, `#[cfg(test)] mod tests` block only.
- Change: add two tests, each stating the bug it catches:
  - `bucket empties and refills` — drain the bucket, advance a fake clock,
    assert tokens return at the configured refill rate; catches a limit
    that never actually applies.
  - `bucket doesn't exceed capacity` — assert a burst of attempts beyond
    capacity is throttled rather than all slipping through; catches a burst
    of up to 300 attempts sliding through unthrottled.
- Verification:
  - `cargo test feed::` — both new tests pass individually by name.
  - Full `cargo test` — Phase 1 + Phase 2's tests still pass unedited,
    report the actual new total.
- Done when:
  - Both tests exist with the names above and pass individually.

### 7. Phase 3 live proxy check and verification gate

- Files or areas: none (verification-only task).
- Change: none.
- Verification:
  - Repeat Phase 2's proxy-kill/restore observation and confirm reconnect
    behaviour is visibly unchanged in the common single-outage case — the
    bucket should be invisible under normal backoff cadence per its own
    design comment. If reconnects visibly slow down or stall under this
    check, that is evidence the capacity/refill numbers are wrong, not that
    the check itself is unnecessary — report the actual observation either
    way.
  - `git diff main --stat -- src/merge.rs` — zero diff (checkpoint 3 of 6).
  - `git diff main --stat` overall — confirm only `src/feed.rs` and
    `src/exchange/mod.rs` have changed since Phase 2.
- Done when:
  - The documented per-venue rate ceiling composes with backoff without
    changing observed reconnect behaviour in the common case, both bucket
    tests pass, and both checkpoint diffs confirm scope. This lands as its
    own commit (e.g. `feed: per-venue token bucket`).

## Phase 4: staleness filter and per-venue thresholds — `src/aggregator.rs`, `src/exchange/mod.rs`

### 8. Add `Venue::staleness_threshold` and filter stale venues before merge

- Files or areas: `src/exchange/mod.rs`, `src/aggregator.rs`.
- Change:
  - `src/exchange/mod.rs`: `impl Venue { fn staleness_threshold(self) ->
    Duration; }`, exhaustive `match`, next to `connect_rate`. Binance: a
    tight threshold in the 1-2s range (it publishes a full snapshot every
    ~100ms regardless of change, so silence itself means failure — a couple
    of missed heartbeats' grace is enough). Bitstamp: **8 seconds**, the
    already-measured value from spec.md's Open Questions (792 messages over
    ~5.25 minutes, max observed gap 1.795s, 4x that rounded up) — implement
    this as a settled input, do not re-measure or re-derive it.
  - `src/aggregator.rs`: before calling `merge::merge(&venues)`, filter the
    `BTreeMap<Venue, &Book>` by `now.duration_since(s.last_update) <
    v.staleness_threshold()`, with `let now = Instant::now();` hoisted once
    per pass, outside the filter closure — so every venue in the same tick
    is judged against the identical instant, not a slightly different one
    per venue.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build` — clean.
  - Covered together with task 9's tests below.
- Done when:
  - `Venue::staleness_threshold` exists with Binance tight and Bitstamp at
    exactly 8 seconds, and the `BTreeMap<Venue, &Book>` handed to
    `merge::merge` is pre-filtered by that threshold using a single
    hoisted `now`.

### 9. Staleness unit tests

- Files or areas: `src/aggregator.rs`, `#[cfg(test)] mod tests` block only
  (and `src/exchange/mod.rs`'s own test module if `staleness_threshold`
  needs a direct assertion of its returned values).
- Change: add three tests, each taking an injected `Instant` rather than
  calling `Instant::now()` internally, each stating the bug it catches:
  - `stale venue excluded from merge` — a venue past its threshold is
    absent from the map handed to `merge()`; catches staleness not being
    wired into the aggregator at all.
  - `fresh venue included` — a venue just under its threshold survives the
    filter; catches a threshold set too tight, dropping everything.
  - `thresholds differ per venue` — Binance's and Bitstamp's thresholds are
    asserted as distinct values; catches the two collapsing to one shared
    value.
- Verification:
  - `cargo test aggregator::` (and `cargo test exchange::` if applicable) —
    all three pass individually by name.
  - Full `cargo test` — Phase 1-3's tests still pass unedited, report the
    actual new total.
- Done when:
  - All three tests exist with the names above and pass individually.

### 10. Phase 4 live proxy check (core acceptance criterion) and verification gate

- Files or areas: none (verification-only task).
- Change: none.
- Verification:
  - **`git diff main --stat -- src/merge.rs` — zero diff, checkpoint 4 of 6
    and the single most important checkpoint in this whole task list.**
    This is the phase where the temptation to fold staleness logic into
    `merge()` is highest, since staleness is itself time-based. Any nonzero
    diff here is a stop-and-flag condition, not a design call to resolve
    unilaterally.
  - **Live proxy-interruption check — the packet's core acceptance
    criterion, run here for real for the first time.** With the full binary
    running against the live proxy (a real client attached via
    `cargo run --bin client` if Phase 6 has landed, otherwise `grpcurl`
    streaming — note explicitly which was used), kill the proxy and wait
    past Binance's threshold (the tight one; the proxy carries Binance's
    connection in this project's setup). Confirm the streamed `Summary`
    narrows to Bitstamp-only levels rather than continuing to publish stale
    Binance prices. Restore the proxy and confirm Binance levels return
    once reconnected (Phase 2) and judged fresh again (this phase). Quote
    the actual observed `grpcurl` (or client) output at each stage — not
    just "it worked."
  - `git diff main --stat` overall — confirm only `src/aggregator.rs` and
    `src/exchange/mod.rs` have changed since Phase 3.
- Done when:
  - A demonstrably stale venue's book never reaches `merge()`, the three
    unit tests pass, `src/merge.rs` shows zero diff, and the live proxy-kill
    test shows the combined book actually narrowing in real gRPC output.
    This lands as its own commit (e.g. `aggregator: staleness filter`).

## Phase 5: grace period for never-seen data — `src/aggregator.rs`, `src/main.rs`

### 11. Add the grace-period exit and confirm the supervisor's fatal/non-fatal split

- Files or areas: `src/aggregator.rs`, `src/main.rs`.
- Change:
  - `src/aggregator.rs`: track `started_at: Instant` for the aggregator
    task. If `started_at.elapsed() > GRACE && self.venues.is_empty()`, log
    an error naming the configured pair and return, making the
    aggregator's own return fatal again. `GRACE = Duration::from_secs(60)`
    — the already-confirmed value from spec.md's Open Questions (covers one
    full backoff cycle, `1+2+4+8+16+30 = 61s`); implement it as a settled
    input, do not re-derive it.
  - `src/main.rs`: read through the Phase 1 `JoinSet` loop and confirm the
    `Component::Aggregator` and `Component::Server` arms still exit the
    process on return/error (unchanged since Phase 1), while
    `Component::Feed(_)` returning is no longer treated as fatal — since
    Phase 2, a feed's own loop no longer returns except on panic, so this
    task is confirming the supervisor's exit-policy statement matches
    reality after Phases 2, 4, and 5 combined, not adding new branching
    logic to `main.rs`.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build` — clean.
  - Covered together with task 12's test and task 13's live checks below.
- Done when:
  - The aggregator exits with a named-pair error message after `GRACE` with
    an empty venue map, and `src/main.rs`'s exit-policy statement (feed
    return non-fatal, aggregator/server return fatal) is confirmed accurate
    by reading the current code, not assumed to still hold from Phase 1.

### 12. Grace-period unit test

- Files or areas: `src/aggregator.rs`, `#[cfg(test)] mod tests` block only.
- Change: add `empty map past grace exits` — with an injected clock placed
  past `GRACE` and an empty venue map, assert the aggregator signals exit.
  This is piece 6's core guarantee per spec.md's Testing Strategy — the test
  it names explicitly.
- Verification:
  - `cargo test aggregator::empty_map_past_grace_exits` (or its actual
    generated identifier) — passes individually.
  - Full `cargo test` — Phase 1-4's tests still pass unedited, report the
    actual new total.
- Done when:
  - The test exists with that name and passes.

### 13. Phase 5 live checks (bad pair, and a mid-reconnect valid pair) and verification gate

- Files or areas: none (verification-only task).
- Change: none.
- Verification:
  - Real-binary check: `cargo run -- --pair xyzabc --port 50051` (a pair
    Bitstamp accepts without validating and produces nothing for). Confirm
    the process exits within roughly 60s with a log message naming the
    pair. This is one of spec.md's explicit Acceptance Criteria, run here
    directly, not only inferred from the unit test.
  - Live proxy check, extended: with a **valid** pair (`ethbtc`) and the
    proxy live, kill the proxy for longer than the grace period and confirm
    the process does **not** exit — Bitstamp keeps producing data, so
    `venues` stays non-empty and the grace-period branch never fires. This
    is the concrete distinction between "one venue reconnecting" and "no
    venue ever produced data," proven live, not just reasoned about.
  - `git diff main --stat -- src/merge.rs` — zero diff (checkpoint 5 of 6).
  - `git diff main --stat` overall — confirm only `src/aggregator.rs` and
    `src/main.rs` have changed since Phase 4.
- Done when:
  - A genuinely bad pair name is observed exiting within the grace period
    with a clear log message, a valid pair with one venue mid-reconnect is
    observed staying up, and both checkpoint diffs confirm scope. This
    lands as its own commit (e.g. `aggregator: grace period for never-seen
    data`).

## Phase 6: client header venue status, README, `compose.yml`, full verification gate

### 14. Client-side per-venue last-seen tracking and header render

- Files or areas: `src/bin/client.rs` only.
- Change:
  - A client-side map (e.g. `HashMap<Venue, Instant>`) tracking last-seen
    -in-a-frame per venue, updated whenever a streamed `Summary` contains at
    least one level from that venue. Without this there is no duration to
    display, only presence/absence — this is a spec correction, not
    cosmetic.
  - Fill in the header's venue-status field, using the venue-list-taking
    render function shape already set up in the prior client step for this
    purpose: `●` for a venue seen in the most recent frame, `○ stale <Ns>`
    for one that is not, with `<Ns>` computed from the client's own
    last-seen map on every redraw — not from any server-side signal (the
    wire schema carries no venue-health field and is not touched here).
  - Do not add a test file or a `#[test]` for `client.rs` — matching the
    prior client step's standing convention that this is a rendering/manual
    -verification surface, not a logic surface.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build` — clean.
  - Manual, real-path run: `cargo run -- --pair ethbtc --port 50051` in one
    terminal, `cargo run --bin client -- --addr http://127.0.0.1:50051` in
    another. Observe both venues showing `●` under normal operation.
- Done when:
  - The client renders live per-venue status with a duration computed from
    its own last-seen tracking, confirmed against a real running server —
    the full kill/restore observation is deferred to task 17's full gate,
    this task confirms the steady-state render only.

### 15. README pass

- Files or areas: `README.md` only.
- Change:
  - Reconnection section: jittered backoff and — explicitly, not just the
    mechanism — *why* the reset waits for stability (the thrash-pattern
    reasoning from Phase 2).
  - Staleness section: per-venue thresholds, and where Bitstamp's 8s number
    came from — the actual measured numbers (792 messages, 1.795s max
    observed gap), not just the chosen threshold.
  - Grace period section: 60s and the backoff-cycle reasoning
    (`1+2+4+8+16+30=61s`).
  - Connection rate limits section: Binance's documented 300-per-5-minute
    ceiling and Bitstamp's explicitly-labelled undocumented conservative
    guess.
  - Production section: a note that venue health belongs on the wire in a
    real system, naming the concrete blind spot — a venue publishing
    normally but never making the top 10 looks identical to a genuinely
    stale venue — as the stated reason, not as a general principle.
  - Fix both places that currently claim the container exits shortly after
    startup if it cannot reach Binance (the Docker section and the
    top-of-file summary — both need checking, not just one) — that
    described the old `select!` exit-on-failure behaviour this packet
    removes.
  - Word budget: README was 1,157 words before this step; this step adds
    the most of any step so far. Keep the result under roughly 1,400 words,
    trimming elsewhere if it would exceed that.
- Verification:
  - Read-through: confirm every new section is present and no sentence
    still describes the old exit-on-any-feed-failure behaviour.
  - `wc -w README.md` — report the actual observed word count and confirm
    it is under ~1,400.
- Done when:
  - Every README claim about reconnection, staleness, the grace period, and
    connection rate limits matches what Phases 1-5 actually shipped, and
    the word count stays within budget.

### 16. `compose.yml` comment fix

- Files or areas: `compose.yml` only.
- Change: read through any comment or documentation line in this file and
  confirm none still describes the old exit-on-any-feed-failure behaviour;
  fix any that do.
- Verification:
  - Read-through, confirm no stale claim remains.
- Done when:
  - No comment in `compose.yml` describes the removed exit-on-failure
    behaviour.

### 17. Full acceptance-criteria live check and final verification gate

- Files or areas: none (verification-only task).
- Change: none.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build`, `cargo test` — all clean; report the actual final test
    count, building on whatever Phases 1-5 added to the baseline of 32.
  - `wc -w README.md` — reconfirm the final word count is under ~1,400.
  - **The packet's full acceptance-criteria live check, run completely
    here, once, end to end:**
    - `cargo run -- --pair ethbtc --port 50051` and `cargo run --bin client
      -- --addr http://127.0.0.1:50051` both live, through the configured
      proxy.
    - Kill the proxy mid-run: confirm the client's header shows Binance
      transitioning from `●` to `○ stale <Ns>` with the displayed duration
      growing, the streamed book visibly narrowing to Bitstamp-only levels,
      and the server process staying up throughout (no exit, no restart).
    - Restore the proxy: confirm Binance's indicator returns to `●` and
      Binance levels reappear in the combined book, without any process
      restart on either binary.
    - Quote the actual observed header lines and timings — this is the
      packet's real acceptance evidence, not a description of what should
      happen.
  - `docker compose up --build` — confirm it survives the same
    proxy-kill/restore cycle without the container exiting. Report actual
    observed behaviour, including any environment limitation (e.g. no
    Docker daemon here), rather than silently omitting the check.
  - `--pair xyzabc` re-run once more here, end to end through the real
    binary, confirming the grace-period exit still holds after all six
    phases are combined.
  - `git diff main --stat -- src/merge.rs` — zero diff (checkpoint 6 of 6,
    final confirmation at the tip).
  - `git diff main --stat` overall — confirm the whole branch touches only
    `src/main.rs`, `src/feed.rs`, `src/exchange/mod.rs`,
    `src/aggregator.rs`, `src/bin/client.rs`, `README.md`, `compose.yml`,
    and `specs/009-resilience/` — no other path (in particular,
    `src/merge.rs`, `src/server.rs`, `src/model.rs`, `Dockerfile`, and
    `rust-toolchain.toml` must all show zero diff at the tip).
- Done when:
  - Every README claim matches what was actually shipped and actually
    observed running it; the client visibly narrates a real proxy
    kill/restore cycle without the reviewer needing to read logs; all six
    `src/merge.rs` checkpoints across the whole branch show zero diff; and
    the full verification gate is clean at the tip. This lands as two
    commits — `client: venue status header` (task 14) and `README:
    reconnection, staleness, grace period, rate limits` (tasks 15-16
    together) — matching the project's standing pattern of a doc pass as
    its own commit, separable from the feature it describes.

## Final Verification

Before closing the packet, run the following once at the tip of the branch
— this is the most representative real-behaviour path for this step (a real
proxy interruption observed through a real client against a real server),
not a rerun of the per-phase checks alone:

- `cargo build && cargo test && cargo clippy --all-targets -- -D warnings &&
  cargo fmt --check`
- `cargo run -- --pair ethbtc --port 50051` and `cargo run --bin client --
  --addr http://127.0.0.1:50051`, both live through the configured proxy:
  kill the proxy, watch Binance go stale in the client header and the
  combined book narrow to Bitstamp-only while the server stays up; restore
  the proxy and watch Binance recover without a process restart. Quote
  actual observed output.
- `cargo run -- --pair xyzabc --port 50051` — confirm exit within the grace
  period with a log message naming the pair.
- `docker compose up --build` — confirm the container survives the same
  proxy interruption without exiting; report actual observed behaviour,
  including any environment limitation.
- `git diff main --stat -- src/merge.rs` — zero diff, final confirmation.
- `git diff main --stat` at the tip — confirm only the files named in
  spec.md's Scope section (plus `compose.yml`'s comment fix) are touched.
