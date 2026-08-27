# Tasks: 012-kraken

**This branch (`012-kraken`) is research-only and is not intended to merge
into `main`.** Every task below produces evidence about what a third,
incremental-update venue costs this codebase; none of it is a claim that the
result ships. Say so again in every commit message on this branch.

## Task Writing Rules

- Each task should describe a real unit of progress.
- Each task should name the expected files or areas touched.
- Each task should include explicit verification.
- Prefer behavior-level verification over mock-only checks.

Six phases below map 1:1 to plan.md's phase breakdown — do not reorder or
merge phases. Phase 1's capture and Phase 3's live measurement are
investigation tasks, not coding tasks: their done-criteria is a written,
evidence-backed conclusion, not a diff. Do not let either be skipped or
replaced with an invented value standing in for the real one.

`git diff main --stat -- src/merge.rs` is checked at the end of every phase
below, expecting zero diff every time — this project's single most
emphasized cross-venue invariant, unaffected by this being a research
branch. Any task whose change plausibly touches `src/aggregator.rs` or
`src/model.rs` (there are none planned, but Phase 2's fixture work is the
one place a `Book`-shape gap could plausibly surface one) must run this same
check as part of its own verification, not defer it to the phase gate.

## Phase 1: live capture — resolve price/qty type and confirm message shapes

### 1. Capture real Kraken traffic and write the price/qty conclusion

- Files or areas: none in `src/`. This is an investigation task — its
  output is a scratch capture file (e.g.
  `specs/012-kraken/inputs/003-kraken-capture.md` or an equivalent scratch
  note) recording what was observed, handed to Phase 2 as input. Do not
  write or edit any file under `src/` for this task.
- Investigation:
  - Connect to `wss://ws.kraken.com/v2` (via this project's existing HTTP
    CONNECT proxy setup, the same mechanism the Bitstamp fixture used) and
    subscribe with
    `{"method":"subscribe","params":{"channel":"book","symbol":["ETH/BTC"],"depth":10}}`
    (or the project's configured pair, converted to Kraken's slash-separated
    uppercase form). Report the actual tool used to capture traffic (a
    throwaway script, `websocat`, or equivalent) — do not assert a tool was
    used without naming it.
  - Capture at least one real `type: "snapshot"` message and at least one
    real `type: "update"` message verbatim, long enough to inspect the
    `bids`/`asks`/`checksum` field shapes directly.
  - Capture at least one real `heartbeat` message and one real `status`
    message. Attempt to trigger a `success: false` subscribe ack (e.g. a
    malformed symbol); if this isn't practically triggerable live within
    reasonable effort, say so plainly and note that the fixture for that
    case will be a documented best-effort construction, not a live capture.
- Done-criteria (written conclusion, not a test or a diff):
  - An explicit sentence stating either "prices/quantities are numbers" or
    "prices/quantities are decimal strings," quoting the actual captured
    `bids`/`asks` field values as evidence — not a restatement of either
    Kraken doc page's claim.
  - The captured `snapshot`, `update`, `heartbeat`, and `status` messages
    recorded verbatim (trimmed if unwieldy, per this project's fixture
    convention) alongside that conclusion, ready for Phase 2 to build real
    fixtures from directly.
  - If the `success: false` ack could not be captured live, an explicit
    statement to that effect, distinct from the messages that were captured.
- Rollback boundary: none — no source diff from this task.

## Phase 2: `src/exchange/kraken.rs` — the `Exchange` impl

### 2. Add the CRC32 dependency

- Files or areas: `Cargo.toml`, `Cargo.lock`.
- Change: confirm no crate already in the dependency tree provides CRC32
  (read the current `Cargo.toml` before adding anything), then `cargo add
  crc32fast`. Record the exact version `cargo add` resolves — do not assume
  one from memory.
- Verification:
  - `cargo build` — clean, confirms the dependency resolves.
  - `git diff main --stat -- Cargo.toml Cargo.lock` — confirm only the new
    dependency entries appear, nothing else changed.
- Done when: `crc32fast` is a direct dependency at a recorded, resolved
  version, and `cargo build` is clean with no other file touched.

### 3. `Kraken` struct, symbol converter, `connect_url`/`subscribe_message`

- Files or areas: `src/exchange/kraken.rs` (new file).
- Change:
  - `pub struct Kraken { book: RefCell<Option<Book>> }`, matching the
    architecture already resolved in this packet's spec — a loud doc
    comment on the struct itself (not deferred to the `impl Exchange`
    block only) stating plainly that `parse` is order-dependent for this
    type: calling it twice with the same `update` message would silently
    double-apply that delta, unlike every other venue in this codebase.
  - A general lowercase-token-to-slash-separated-uppercase symbol converter
    (e.g. `"ethbtc" -> "ETH/BTC"`), splitting on a known quote-currency
    suffix (`btc`, `usd`, `eur`, and whatever other suffixes this project's
    existing `--pair` values already assume are the only quote currencies).
    Implemented and tested in this file only — not extracted to a shared
    module, since nothing else needs it.
  - `connect_url(&self, _pair: &str) -> String` returning
    `"wss://ws.kraken.com/v2"` (pair unused, matching Bitstamp's precedent).
  - `subscribe_message(&self, pair: &str) -> Option<String>` building the
    `book` channel subscribe JSON using the symbol converter's output.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build` — clean.
  - `cargo test exchange::kraken::` — the symbol converter's own unit test
    (e.g. `symbol converter round-trips the project's default pair`) passes
    individually by name; report the actual generated test identifier.
  - `git diff main --stat -- src/merge.rs` — zero diff.
- Done when: `kraken.rs` compiles with the struct, its loud doc comment, the
  symbol converter, and the two straightforward trait methods; the
  converter's own test passes.

### 4. `parse` dispatch, `RefCell` accumulation, CRC32 checksum

- Files or areas: `src/exchange/kraken.rs`.
- Change:
  - `parse(&self, raw: &str) -> Option<Book>`: two-level dispatch on the
    top-level `"channel"` field (`"book"` vs `"heartbeat"` vs `"status"`)
    before inspecting `"type"`, plus a third branch for
    `{"method":"subscribe","success":...}` acks — `success: false` logged
    at `warn` (matching this project's precedent for the one lifecycle
    message that means something is actually wrong), `success: true` routed
    to `None` silently like the other lifecycle messages.
  - A `snapshot` message replaces `self.book`'s contents wholesale. An
    `update` message mutates the held book in place — applying each
    changed level, removing any level whose `qty` is `0` — and returns a
    clone of the resulting full state. If no book is held yet when an
    `update` arrives (e.g. a message ordering the connection shouldn't
    produce, but the code must not panic on), return `None` rather than
    treating it as a snapshot.
  - **Reconnect-state sub-step, explicit — do not treat this as
    automatically covered by "the next snapshot replaces it."** Confirm, by
    reading `src/feed.rs`'s current reconnect loop, whether one `Kraken`
    value (and therefore one `RefCell`) is reused across multiple
    `run_once`/reconnect attempts within a single `run_feed` call, or
    whether a fresh `Kraken` is constructed per attempt. Report which is
    true. If the same `Kraken` is reused across reconnects, the `RefCell`
    must be explicitly reset (e.g. `self.book.borrow_mut().take()` or
    equivalent) at the point a fresh connection is established — not left
    to rely on "the next snapshot will overwrite it" as the only mechanism,
    since a delayed or dropped snapshot after reconnect would otherwise let
    a stale pre-disconnect book keep being returned from `parse` if an
    `update` somehow arrived before the first post-reconnect `snapshot`.
    State explicitly in a code comment which of these two cases applies and
    why the chosen handling is correct for it.
  - CRC32 checksum: after every `parse` call that produces a book (both
    `snapshot` and `update` paths), compute the checksum per Kraken's
    documented algorithm (top 10 asks low to high, then top 10 bids high to
    low, digit-strings with `.` and leading zeros stripped, concatenated,
    CRC32'd via `crc32fast`) and compare against the message's own
    `checksum` field. On mismatch: log a warning and clear `self.book`
    (`.take()`), forcing the next message to wait for a fresh `snapshot`
    rather than continuing to accumulate from state already known to have
    diverged.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build` — clean.
  - `git diff main --stat -- src/merge.rs` — zero diff.
  - `git diff main --stat -- src/aggregator.rs` — zero diff (this task is
    the one most likely in this packet to tempt reaching into the
    aggregator for a "handle partial updates" special case; confirm it
    didn't happen).
- Done when: `parse` dispatches correctly on all four message shapes,
  accumulates `update`s onto a held `snapshot`, clears state on checksum
  mismatch, the reconnect-state sub-step's finding is recorded in a code
  comment, and both diff checks above are zero.

### 5. Fixture tests built from Phase 1's capture

- Files or areas: `src/exchange/kraken.rs`, `#[cfg(test)] mod tests` block
  only.
- Change: add tests using Phase 1's real captured messages (not hand-built
  JSON), each named as a behaviour sentence stating the bug it catches:
  - `a captured snapshot parses into a complete book` — asserts the
    snapshot's bid/ask levels land in the returned `Book` correctly, not
    just that parsing didn't panic.
  - `a captured update accumulates onto the held snapshot` — feed the
    captured `snapshot` then the captured `update` through the same
    `Kraken` instance and assert the resulting book reflects the update's
    changed levels specifically (a level's price/amount changed as
    expected) — the one new test shape in this packet per this project's
    testing convention; a single-message smoke test alone is not sufficient
    evidence for stateful accumulation.
  - `a qty of zero removes that price level` — an `update` fixture (real or
    a minimally hand-modified copy of the real one, documented as such)
    containing a `qty: 0` entry results in that level being absent from the
    next returned book.
  - `heartbeat parses to none without panicking`, `status parses to none
    without panicking`, `a false subscribe ack parses to none and logs a
    warning` — three separate tests, one per control-message shape, mapped
    to Phase 1's captures (or Phase 1's documented best-effort construction
    for the `success: false` case if a live capture wasn't obtained).
  - `calling parse twice with the same update double-applies the delta` —
    the concrete evidence for task 3's loud doc comment: feed the same
    captured `update` through `parse` twice on one `Kraken` instance and
    assert the second call's resulting amounts reflect the delta having
    been applied a second time (not idempotent) — this is the test that
    turns the doc comment's claim into a checked fact rather than an
    assertion.
  - `a corrupted checksum clears the held book` — take a real captured
    fixture pair and deliberately corrupt one digit of the `checksum`
    field; assert `parse` logs a warning and that a subsequent `update`
    (without an intervening `snapshot`) returns `None` rather than
    continuing to accumulate — constructible without a live server, since
    this is pure state-machine behavior over already-captured fixtures.
- Verification:
  - `cargo test exchange::kraken::` — every test above passes individually
    by name; report the actual generated identifiers and the exact new
    count.
  - `cargo test` — full suite; report the total against the pre-Phase-1
    baseline, confirm no existing assertion needed editing.
  - `git diff main --stat -- src/merge.rs` — zero diff.
- Done when: every fixture test above exists, is built from Phase 1's real
  captured data (or its explicitly documented best-effort fallback), passes
  individually, and the double-`parse` test gives concrete evidence for the
  struct's loud doc comment rather than leaving it as an unverified claim.

### 6. Phase 2 verification gate

- Files or areas: none (verification-only task).
- Change: none.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build`, `cargo test` — all clean; report the final test count.
  - `git diff main --stat -- src/merge.rs` — zero diff (checkpoint 1 of 3).
  - `git diff main --stat -- src/aggregator.rs` — zero diff (checkpoint 1 of
    1 for this file across the whole packet, since no later phase touches
    it either).
  - `git diff main --stat` overall — confirm only `src/exchange/kraken.rs`
    (new), `Cargo.toml`, `Cargo.lock`, and `specs/012-kraken/` are touched.
- Done when: `kraken.rs` compiles, its fixture tests (built from real
  captured data) pass, the checksum and accumulation logic are exercised by
  a real multi-message sequence rather than a single-message smoke test,
  and both invariant-file diffs are zero. This lands as its own commit
  (e.g. `exchange: Kraken parser, RefCell accumulation, CRC32 checksum`).

## Phase 3: `src/exchange/mod.rs` — `Venue::Kraken`, `connect_rate`, staleness

### 7. Live-measure Kraken's staleness window

- Files or areas: none in `src/`. Investigation task — output is a scratch
  note or the commit message for task 8, recording the measurement.
- Investigation:
  - Hold a live Kraken `book` connection open for a window comparable in
    spirit to this project's prior venue staleness measurement (multiple
    minutes, hundreds of messages) and record every gap between consecutive
    messages. Decide, based on Phase 1's capture of `heartbeat` cadence,
    whether `heartbeat` messages count as liveness signals for this
    measurement or only `update`/`snapshot` messages do — state which was
    used and why.
  - Record the actual message count, the window length, and the maximum
    observed gap.
- Done-criteria (written conclusion, not a test or a diff):
  - An explicit reported measurement: message count, window length in
    seconds/minutes, and the maximum observed gap in seconds — with the
    same evidentiary shape this project's Bitstamp staleness figure used
    (a specific number of messages over a specific window, not "seemed
    stable").
  - A stated threshold derived from that measurement as a documented
    multiple of the observed max gap (e.g. "4x observed max, rounded up"),
    with the reasoning written down, ready for task 8 to encode directly —
    not re-derived or guessed at task 8's time.
- Rollback boundary: none — no source diff from this task.

### 8. `Venue::Kraken`, `connect_rate()`, `staleness_threshold()`

- Files or areas: `src/exchange/mod.rs`.
- Change:
  - Append `Kraken` as the **third** `Venue` variant, after `Bitstamp` —
    appending, not inserting anywhere else, so the existing two venues'
    `BTreeMap` iteration order and tie-break behavior are unchanged.
  - `connect_rate()` arm for `Kraken`: no documented public-WebSocket
    connection-rate limit was found during this packet's research — encode
    a conservative guess with a comment stating plainly it is a guess, not
    a documented fact, matching the existing treatment already present for
    Bitstamp's entry.
  - `staleness_threshold()` arm for `Kraken`: the number and reasoning from
    task 7's written conclusion, encoded as a `Duration` with a code
    comment stating the measured message count, window, and max observed
    gap it was derived from — do not encode a placeholder or a
    docs-transcribed number here.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build` — clean.
  - Covered together with task 9's tests below.
- Done when: `Venue::Kraken` compiles into both exhaustive `match`es, and
  `staleness_threshold()`'s `Kraken` arm is traceable directly to task 7's
  reported measurement via its code comment.

### 9. Extend existing `Venue` tests and Phase 3 verification gate

- Files or areas: `src/exchange/mod.rs`, `#[cfg(test)] mod tests` block
  only.
- Change:
  - Extend the existing per-venue-threshold test (or add a Kraken-specific
    variant of it) to assert `Venue::Kraken`'s staleness threshold differs
    from both `Venue::Binance`'s and `Venue::Bitstamp`'s.
  - Extend the existing venue-display test to cover
    `Venue::Kraken.to_string() == "kraken"`.
- Verification:
  - `cargo test exchange::` — both extended/added assertions pass
    individually; report the actual new total against Phase 2's baseline.
  - `cargo test` — full suite, confirm no existing assertion needed
    editing.
  - `git diff main --stat -- src/merge.rs` — zero diff (checkpoint 2 of 3).
  - `git diff main --stat` overall — confirm only `src/exchange/mod.rs` has
    changed since Phase 2.
- Done when: both extended tests pass, `Venue::Kraken`'s threshold is
  distinct from the other two venues' and traceable to a real measurement,
  and the diff check is zero. This lands as its own commit (e.g.
  `exchange: Venue::Kraken, connect_rate, measured staleness threshold`).

## Phase 4: `src/feed.rs` — Kraken-specific proactive ping

### 10. Idle-timer branch sending a client-initiated ping

- Files or areas: `src/feed.rs`.
- Change:
  - Track the time of the last message received on the current connection
    inside `run_feed`'s read loop. Prefer a branch gated on
    `exchange.venue() == Venue::Kraken` inside the existing loop over
    widening the `Exchange` trait with a new opt-in method — the trait
    stays untouched, and Binance/Bitstamp's read path shows no behavioral
    change, per this packet's explicit framing that this is a
    Kraken-specific concern, not a shared one. If implementation finds a
    concrete reason the trait-level version is meaningfully simpler, record
    that reasoning explicitly before choosing it instead.
  - When idle past ~30s on a Kraken connection specifically, send
    `Message::Text(r#"{"method":"ping"}"#.into())`.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build` — clean.
  - Covered together with task 11's test below.
- Done when: a Kraken connection idle past the threshold sends a
  client-initiated ping, and the branch is confined to the
  `venue() == Venue::Kraken` (or equivalent) condition.

### 11. Idle-timer unit test and Phase 4 verification gate

- Files or areas: `src/feed.rs`, `#[cfg(test)] mod tests` block only.
- Change: add a test expressing the idle-timer decision as a pure function
  of "time since last message" vs. threshold (matching this project's
  existing injected-clock testing pattern in this file), not requiring a
  live socket — e.g. `an idle kraken connection past threshold triggers a
  ping decision`, stating the bug it catches: the timer never firing, or
  firing for every venue instead of Kraken alone.
- Verification:
  - `cargo test feed::` — existing backoff/jitter/token-bucket tests pass
    unedited (confirming this phase didn't touch shared reconnect logic),
    plus the new idle-timer test passing individually by name.
  - `cargo test` — full suite; report the new total.
  - `git diff main --stat -- src/merge.rs` — zero diff.
  - `git diff main --stat -- src/exchange/binance.rs src/exchange/bitstamp.rs`
    — zero diff on both, confirming the branch is genuinely Kraken-scoped.
  - `git diff main --stat` overall — confirm only `src/feed.rs` (and, only
    if the trait-opt-in path was chosen instead of the venue-gated branch,
    `src/exchange/mod.rs`) has changed since Phase 3.
- Done when: the idle-timer test passes, the existing feed tests are
  unedited, and both Binance/Bitstamp files show zero diff. This lands as
  its own commit (e.g. `feed: Kraken-specific idle ping`). The live
  confirmation that this actually keeps a Kraken connection open past 60s
  is deferred to Phase 6, not claimed here from the unit test alone.

## Phase 5: `src/main.rs` — wire the third feed spawn

### 12. Add the third `feed::run_feed(Kraken, ...)` spawn and full local run

- Files or areas: `src/main.rs`.
- Change: clone `feed_tx` and `pair` a third time; add one more
  `tasks.spawn(async move { ... (Component::Feed(Venue::Kraken), res) })`
  block, mechanically identical to the existing Binance/Bitstamp spawns.
- Verification:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build` — clean.
  - `cargo test` — full suite passes unedited; this task adds no new pure
    logic, only wiring.
  - `cargo run -- --pair ethbtc --port 50051` with `RUST_LOG=debug`: report
    the actual observed log lines showing Kraken connecting, subscribing,
    and producing book updates alongside Binance's and Bitstamp's existing
    log lines — not just "it started."
  - `grpcurl -plaintext 127.0.0.1:50051 orderbook.OrderbookAggregator/BookSummary`
    against the same live run: confirm at least one streamed `Summary`
    contains a `"kraken"`-labelled level once Kraken's book has populated,
    alongside `"binance"`/`"bitstamp"` levels — quote the actual observed
    response.
  - `git diff main --stat -- src/merge.rs` — zero diff (checkpoint 3 of 3).
  - `git diff main --stat` overall — confirm only `src/main.rs` has changed
    since Phase 4.
- Done when: a local `cargo run` streams a real three-venue combined book
  over gRPC, observed directly via `grpcurl`, and the diff check is zero.
  This lands as its own commit (e.g. `main: wire Kraken feed spawn`).

## Phase 6: live verification — reconnect, staleness, three-venue merge

No source changes are expected in this phase. If a check below surfaces a
real bug, fix it as a small, scoped diff to whichever of Phases 2-4's files
is responsible, note the fix explicitly in the report, and re-run the
specific check that failed.

### 13. Forced disconnect: confirm re-subscribe and discarded local state

- Files or areas: none expected (see phase note above for the exception).
- Verification:
  - With the full binary running live (all three venues), interrupt the
    Kraken connection specifically (kill the proxy if Kraken is tunneled
    through it in this environment, or interrupt the socket another way).
    Confirm, via log lines, that Kraken re-subscribes on reconnect.
  - Confirm the first book published after reconnect reflects only the
    fresh post-reconnect `snapshot` onward — not a resumed pre-disconnect
    state. Report the actual observed sequence of log lines and, if
    practical, the `grpcurl` output immediately before and after the
    disconnect.
  - `git diff main --stat -- src/merge.rs` — zero diff.
- Done when: a real observed reconnect shows re-subscribe and discarded
  prior state, reported with actual log/output evidence, not inferred.

### 14. Quiet-Kraken staleness exclusion

- Files or areas: none expected.
- Verification:
  - With all three venues live and fresh, block Kraken's connection
    specifically (not Binance/Bitstamp) for longer than task 8's measured
    threshold.
  - Via `grpcurl`, confirm the streamed `Summary` narrows to
    binance/bitstamp-only levels while Kraken is blocked.
  - Restore Kraken's connection and confirm `"kraken"` levels reappear once
    fresh, quoting the actual observed responses at each stage.
  - `git diff main --stat -- src/merge.rs` — zero diff.
- Done when: a real observed staleness exclusion and recovery is reported
  with actual `grpcurl` output, not inferred from the unit-level staleness
  test alone.

### 15. Full three-venue merge

- Files or areas: none expected.
- Verification:
  - With all three feeds live and fresh, confirm via `grpcurl` that at
    least one streamed `Summary` contains all three exchange labels
    (`"binance"`, `"bitstamp"`, `"kraken"`) among its top-10 bids/asks.
    Report the actual observed labels and a sample of the levels, not just
    "it worked."
  - `git diff main --stat -- src/merge.rs` — zero diff.
- Done when: a real observed three-venue combined `Summary` is reported
  with actual output quoted.

### 16. Checksum-mismatch recovery path

- Files or areas: none expected, unless task 4's checksum-clear behavior is
  found to need a fix, in which case the fix is scoped to
  `src/exchange/kraken.rs` only.
- Verification:
  - If triggerable live (e.g. by forcing a brief network hiccup that
    corrupts or drops a message mid-stream), confirm a checksum mismatch
    logs a warning and that the venue produces no further books until the
    next `snapshot` (via reconnect or re-subscribe). Report which behavior
    was actually observed.
  - If not triggerable live within reasonable effort, cite task 5's
    corrupted-fixture unit test explicitly as the fallback evidence and say
    so plainly — do not claim a live observation that didn't happen.
  - Either way, state explicitly whether a checksum mismatch without a
    reconnect leaves Kraken silently producing no further books — the
    uncertainty this packet's spec flagged as worth confirming rather than
    asserting.
  - `git diff main --stat -- src/merge.rs` — zero diff.
- Done when: the checksum-mismatch recovery question has a real, reported
  answer (live-observed or explicitly the fixture-test fallback), not left
  as an open assertion.

### 17. Final verification gate

- Files or areas: none.
- Verification:
  - `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D
    warnings`, `cargo fmt --check` — all clean at the tip; report the final
    test count against the pre-Phase-1 baseline.
  - `git diff main --stat -- src/merge.rs` — zero diff, final confirmation
    at the tip of the branch.
  - `git diff main --stat` at the tip — confirm the whole branch touches
    only `src/exchange/kraken.rs` (new), `src/exchange/mod.rs`,
    `src/feed.rs`, `src/main.rs`, `Cargo.toml`, `Cargo.lock`, and
    `specs/012-kraken/` — in particular, `src/exchange/binance.rs`,
    `src/exchange/bitstamp.rs`, `src/merge.rs`, `src/model.rs`,
    `src/aggregator.rs`, `src/server.rs`, and `proto/orderbook.proto` must
    all show zero diff.
  - Reconfirm every commit message on this branch states plainly that it
    does not merge into `main`.
- Done when: the full local test/lint suite is clean, the invariant-file
  diffs are all zero at the tip, and tasks 13-16's live evidence is
  gathered and reported. This is the branch's final gate, not a further
  build-out step.

## Final Verification

Before considering this packet's work done, run the following once at the
tip of the branch — the most representative real-behaviour path for this
step (a real three-venue system observed end to end through `grpcurl`), not
a rerun of the per-phase checks alone:

- `cargo build && cargo test && cargo clippy --all-targets -- -D warnings &&
  cargo fmt --check`
- `cargo run -- --pair ethbtc --port 50051` live, with all three venues
  connected: `grpcurl -plaintext 127.0.0.1:50051
  orderbook.OrderbookAggregator/BookSummary` showing `"binance"`,
  `"bitstamp"`, and `"kraken"` levels together in one streamed `Summary`.
  Quote the actual observed response.
- A forced Kraken disconnect and recovery, observed live, showing
  re-subscribe and discarded prior state (task 13), and a forced Kraken
  staleness window, observed live, showing the combined book narrowing and
  recovering (task 14).
- `git diff main --stat -- src/merge.rs` — zero diff, final confirmation.
- `git diff main --stat` at the tip — confirm only the files named in task
  17 are touched.
- A plain restatement, in the final report, that this branch is
  research-only and is not being merged into `main`.
